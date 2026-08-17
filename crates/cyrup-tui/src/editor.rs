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
//! ([`build_visual_line_map`]) and the cursor moves by *visual* line, preserving a **sticky preferred
//! column** ([`InputEditor::preferred_visual_col`]) across short/long/rewrapped lines, falling through
//! to history at the first visual line and to line-end at the last (spec/tui/03 §4.1-§4.2). Large
//! pastes collapse to atomic `[paste #N …]` markers ([`InputEditor::handle_paste`]) that expand back
//! to content on submit ([`InputEditor::expanded_text`], spec/tui/03 §5.5).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use unicode_segmentation::UnicodeSegmentation;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};   // editor.rs:26 — CMDHINT_01 needs the named type
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::autocomplete::{Autocomplete, CompletionContext};
use crate::commands::CommandRegistry;
use crate::component::Component;
use crate::keymap::{EditorAction, EditorKeymap};
use crate::theme::UiTheme;

/// History ring capacity (`editor.ts:381`).
const HISTORY_CAP: usize = 100;

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
    /// Cached whole-tree file list for `@`-mention search (`autocomplete.ts` populates once, then
    /// fuzzy-filters in-process per keystroke). Lazily built on the first `@`-mention, invalidated on
    /// `set_cwd`. `None` until first needed.
    mention_files: Option<Vec<String>>,
    /// The layout width (in columns) used to wrap logical lines into **visual** lines for vertical
    /// motion (`editor.ts:1690` `build_visual_line_map(width)`). Updated every render; `80` until the
    /// first render. Vertical Up/Down resolve against the visual map computed at this width.
    view_width: usize,
    /// First **visual** line of the render window (`editor.ts:288` `scrollOffset`). The editor shows
    /// at most `maxVisibleLines` rows; anything above/below is scrolled out and announced by the
    /// `─── ↑ N more ` / `─── ↓ N more ` rules ([`scroll_border`], `editor.ts:259-268`). Kept in range
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
    /// Large-paste store (`editor.ts:81` `pastes: id -> expanded content`): each entry is the full
    /// pasted text the buffer shows collapsed to a `[paste #N …]` marker. [`expanded_text`] substitutes
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

impl Default for InputEditor {
    fn default() -> Self {
        InputEditor::new()
    }
}

impl InputEditor {
    /// The command-token highlight + argument-hint ghost for the current buffer (CMDHINT_01).
    ///
    /// Pure: a function of `lines[0]` and the registry alone, recomputed per render. NOT cached
    /// alongside `self.autocomplete` — the popup's lifetime is the very thing this outlives, and 26
    /// state-replacing paths never call `update_autocomplete()` at all: `set_text_internal`
    /// (`:486-495` ← `:1469`, `:1500`, `:1504` history recall **and `:480`**, deliberately, per
    /// TUI-061), its public caller `set_text` (`:472-481`, 8 app call sites — `app/tree_nav.rs:199`,
    /// `app/session_bind.rs:327`, `app/channels.rs:149`, `app/extension_ui.rs:320`,
    /// `app/selectors.rs:395`, `app/crossterm.rs:148`, `extension_editor.rs:58`, `:292`), and
    /// `set_registry` (`:326-328`), which fires on session rebind/`/reload` (`app/run.rs:400`),
    /// session swap (`app/run_arms.rs:33`) and the `enableSkillCommands` settings toggle
    /// (`app/execute_misc.rs:217`). A cached copy would describe a buffer or a registry that no longer
    /// exists — and `close_selector` (`app/selectors.rs:390-397`) can replace both around one frame.
    ///
    /// Scope is **logical line 0** only, and the token boundary is `split_command`'s
    /// (`cyrup-resources/src/prompt.rs:178-190`, pi `prompt-templates.ts:271`): the leading run of
    /// non-whitespace chars after `/`, where the implicit `\n` ending line 0 IS whitespace.
    pub fn command_highlight(&self) -> Option<CommandHighlight> {
        let line0 = self.lines.first()?;
        if line0.first() != Some(&'/') {
            return None;
        }
        // The token boundary: first whitespace CHAR index in line 0, or — when line 0 has none but the
        // buffer continues — line 0's end, because `split_command` finds that `\n`. `None` means the
        // name is still being typed with nothing after it.
        let boundary = line0
            .iter()
            .position(|c| c.is_whitespace())
            .or_else(|| (self.lines.len() > 1).then_some(line0.len()));
        let head: String = line0.iter().take(boundary.unwrap_or(line0.len())).skip(1).collect();
        match boundary {
            // Still typing the name: highlight while it is an honest prefix. No ghost — the popup is
            // open and already showing the full hint + description.
            None => crate::autocomplete::is_command_prefix(&self.registry, &head)
                .then_some(CommandHighlight { token: 0..line0.len(), ghost: None }),
            // Whitespace follows: the highlight FREEZES on `/name` iff that name is an EXACT registered
            // command — `registry.get`, NOT `match_command`/`dispatch_names`, which hold builtins plus
            // `HIDDEN_COMMANDS` only (`commands.rs:189-197`, `:297-312`, `:429-434`) and would drop
            // every prompt template, extension command and skill. Those still run, expanded
            // server-side by `expand_input_text` (`cyrup-session-svc/src/session.rs:1255-1258`). An
            // unknown or partial name (`/flux `, `/bogus `) gets nothing — honest, and identical to
            // today's silent fallback to a literal prompt (`commands.rs:277`).
            Some(i) => {
                let cmd = self.registry.get(&head)?;
                // The argument zone is "empty" when it holds no NON-whitespace char anywhere in the
                // buffer — line 0 past the token, and every later (soft-newline) line. Whitespace-only
                // is still empty, so `/model  ` with two spaces keeps its ghost.
                let zone_empty = line0.iter().skip(i).all(|c| c.is_whitespace())
                    && self.lines.iter().skip(1).all(|l| l.iter().all(|c| c.is_whitespace()));
                let ghost =
                    zone_empty.then_some(cmd.argument_hint.as_deref()).flatten().map(str::to_string);
                Some(CommandHighlight { token: 0..i, ghost })
            }
        }
    }
}

impl InputEditor {
    /// A fresh, empty, focused editor.
    pub fn new() -> Self {
        InputEditor {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            focused: true,
            keymap: EditorKeymap::default(),
            kill_ring: Vec::new(),
            last_action: LastAction::None,
            undo: Vec::new(),
            history: VecDeque::new(),
            history_index: -1,
            history_draft: None,
            jump: None,
            registry: CommandRegistry::new(),
            autocomplete: None,
            autocomplete_max_visible: crate::select_list::DEFAULT_MAX_VISIBLE,
            autocomplete_keymap: crate::keymap::AutocompleteKeymap::default(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            mention_files: None,
            view_width: 80,
            scroll_offset: 0,
            term_rows: 24,
            preferred_visual_col: None,
            pastes: BTreeMap::new(),
            paste_counter: 0,
            thinking_level: "medium".to_string(),
            thinking_level_owns_border: true,
            padding_x: 0,
            show_hardware_cursor: false,
        }
    }

    /// Set the horizontal editor padding (Pi `CustomEditor.setPaddingX`,
    /// `tui/src/components/editor.ts:370-376`): non-finite → `0`, else `max(0, floor(padding))`.
    /// The `/settings` grid cycles `0..=3` and [`cyrup_config::SettingsManager::set_editor_padding_x`]
    /// clamps the persisted value to that range; this setter reproduces Pi's own coercion so a
    /// hand-edited `editorPaddingX` can never render negative or fractional padding.
    pub fn set_padding_x(&mut self, padding: i64) {
        self.padding_x = padding.clamp(0, u16::MAX as i64) as u16;
    }

    /// The current horizontal padding (Pi `getPaddingX`, `editor.ts:365-367`).
    #[must_use]
    pub fn padding_x(&self) -> u16 {
        self.padding_x
    }

    /// Show/hide the terminal's real cursor (Pi `TUI.setShowHardwareCursor`, `tui/src/tui.ts:346`).
    /// See [`Self::show_hardware_cursor`] for why this lives on the editor in cyrup.
    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        self.show_hardware_cursor = enabled;
    }

    /// Whether the hardware cursor is placed each frame (Pi `getShowHardwareCursor`, `tui.ts:342`).
    ///
    /// Cyrup has no standalone `TUI` object owning terminal state — ratatui's `Terminal` does, and
    /// the ONLY component that asks for a cursor position is this editor
    /// ([`Self::cursor_in`] is the sole `Frame::set_cursor_position` caller in the crate), so the
    /// flag rides here rather than on a `TUI` port that does not exist.
    #[must_use]
    pub fn show_hardware_cursor(&self) -> bool {
        self.show_hardware_cursor
    }

    /// The padding actually applied at render width `width`, clamped exactly as Pi clamps it
    /// (`editor.ts:483-484`: `min(paddingX, floor((width - 1) / 2))`) so the content column can
    /// never be squeezed out of existence on a narrow terminal.
    pub fn effective_padding(&self, width: u16) -> u16 {
        self.padding_x.min(width.saturating_sub(1) / 2)
    }

    /// The text layout width inside `width` columns, after padding (Pi `editor.ts:485-489`):
    /// `contentWidth = max(1, width - 2 * paddingX)`, and one column is reserved for the
    /// end-of-line cursor cell ONLY when there is no padding for it to overflow into.
    ///
    /// **Public because measurement and render must agree** (E15). Pi computes `layoutWidth` ONCE
    /// per `render` and feeds the same number to `this.lastWidth` and to `layoutText()`
    /// (`editor.ts:489-497`), so the row count the container reserves is by construction the row
    /// count the editor draws. cyrup splits those two across `app::region_constraints` (which sizes
    /// the slot) and [`Component::render`] (which fills it), and the former used to measure at a
    /// hardcoded `width - 1` while the latter wrapped at `width - 2 * paddingX`: with
    /// `editorPaddingX > 0` the render produced MORE rows than the slot had, and the surplus — the
    /// caret row included — was clipped. Both sides now call this.
    pub fn layout_width(&self, width: u16) -> u16 {
        let pad = self.effective_padding(width);
        let content = width.saturating_sub(pad.saturating_mul(2)).max(1);
        if pad > 0 { content } else { content.saturating_sub(1).max(1) }
    }

    /// Set the reasoning level driving the editor's rule color (spec/tui/03 §3.3). Called by the app
    /// on `ThinkingLevelChanged` / thinking-selector confirm.
    pub fn set_thinking_level(&mut self, level: impl Into<String>) {
        self.thinking_level = level.into();
    }

    /// Detach this editor's rule from the reasoning level, leaving it on the `borderMuted` role —
    /// the state Pi's shared `Editor` is *born* in (`getEditorTheme().borderColor`, v0.84.1
    /// `theme.ts:1301-1304`) and never leaves unless something reassigns `borderColor`
    /// (`interactive-mode.ts:3990-3993`, which only ever touches the chat editor). Used by the
    /// `ui.editor` extension dialog, whose Pi counterpart is a bare
    /// `new Editor(tui, getEditorTheme(), options)` (`components/extension-editor.ts:70`).
    pub fn use_muted_border(&mut self) {
        self.thinking_level_owns_border = false;
    }

    /// Replace the command registry used for slash autocomplete (rebuilt on `/reload`).
    pub fn set_registry(&mut self, registry: CommandRegistry) {
        self.registry = registry;
    }

    /// The editor keymap (for `/hotkeys` to resolve editor-action key labels, `getEditorKeyDisplay`).
    pub fn keymap_ref(&self) -> &EditorKeymap {
        &self.keymap
    }

    /// Merge a JSON keybindings document into the editor keymap **and** the autocomplete-popup keymap
    /// (R-10-018; the `editor.*` + `tui.autocomplete.*` ids). Called by the binary at boot with the
    /// user's `keybindings.json` so custom editor + popup bindings take effect (item #6).
    /// CFG-038 — the two maps' rejected entries are CONCATENATED rather than short-circuited: the
    /// editor map returning an issue must not stop the autocomplete map from being applied.
    pub fn merge_keybindings_json(
        &mut self,
        json: &str,
    ) -> Result<Vec<crate::KeybindingIssue>, crate::TuiError> {
        let mut issues = self.keymap.merge_json(json)?;
        issues.extend(self.autocomplete_keymap.merge_json(json)?);
        Ok(issues)
    }

    /// Restore both editor-side binding tables to their defaults — TUI-051.
    ///
    /// `merge_json` only *sets* the ids present in the document, so a `/reload` after the user
    /// DELETED an entry would leave its old binding live. Upstream's `rebuild()` replaces rather
    /// than merges: for every id in `definitions`, `userKeys === undefined ?
    /// normalizeKeys(definition.defaultKeys) : normalizeKeys(userKeys)`
    /// (`packages/tui/src/keybindings.ts:187-191` @v0.83.0). Resetting to defaults and then merging
    /// the freshly-read document reproduces that.
    pub fn reset_keybindings_to_defaults(&mut self) {
        self.keymap = EditorKeymap::default();
        self.autocomplete_keymap = crate::keymap::AutocompleteKeymap::default();
    }

    /// Set the max visible rows in the autocomplete dropdown (Pi `autocompleteMaxVisible`, item #6):
    /// clamped to 3–20 and applied to any already-open popup + every future one. Called by the binary
    /// from `settings.autocompleteMaxVisible`.
    pub fn set_autocomplete_max_visible(&mut self, n: u16) {
        self.autocomplete_max_visible = n.clamp(3, 20);
        if let Some(ac) = self.autocomplete.as_mut() {
            ac.list.set_max_visible(self.autocomplete_max_visible);
        }
    }

    /// Install a freshly-computed autocomplete popup, applying the configured `autocompleteMaxVisible`
    /// dropdown height (item #6). Centralises the `self.autocomplete = Some(ac)` assignment so every
    /// entry point (slash / `@`-mention / forced Tab) honours the setting.
    fn open_popup(&mut self, mut ac: Autocomplete) {
        ac.list.set_max_visible(self.autocomplete_max_visible);
        self.autocomplete = Some(ac);
    }

    /// Override the working directory used for path completion (defaults to the process cwd). Clears
    /// the cached `@`-mention file list so the new tree is re-enumerated on the next mention.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
        self.mention_files = None;
    }

    /// Inject the `@`-mention candidate file list directly (test seam / an async populator). Bypasses
    /// the lazy `fd`/walk source.
    pub fn set_mention_files(&mut self, files: Vec<String>) {
        self.mention_files = Some(files);
    }

    /// Focus state (R-10-009 — focused inputs drive the hardware cursor).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Whether an autocomplete popup is currently open.
    pub fn autocomplete_open(&self) -> bool {
        self.autocomplete.is_some()
    }

    /// The active autocomplete popup, if any (for the chrome to render it below the editor).
    pub fn autocomplete(&self) -> Option<&Autocomplete> {
        self.autocomplete.as_ref()
    }

    /// True while the buffer's first line — **after leading whitespace** — begins with `!` (bash mode
    /// → green border, spec/tui/03 §7.1). Mirrors Pi's `text.trimStart().startsWith("!")`
    /// (interactive-mode.ts:2525): a leading indent before `!` (e.g. `  !ls`) still enters bash mode,
    /// matching the dispatcher, which `trim()`s before the `!` check (item #5 "trim_start on bash").
    pub fn is_bash_mode(&self) -> bool {
        self.lines
            .first()
            .and_then(|l| l.iter().find(|c| !c.is_whitespace()))
            .is_some_and(|c| *c == '!')
    }

    /// The full buffer text with `\n` line joins.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    /// Whether the buffer is empty (no chars on any line).
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Vec::is_empty)
    }

    /// Number of buffer lines (≥ 1) — used to size the editor area.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The cursor position as `(row, col)` in chars.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// The prompt history (most-recent first), for inspection/tests.
    pub fn history(&self) -> &VecDeque<String> {
        &self.history
    }

    /// The kill-ring top (most recent killed text), for inspection/tests.
    pub fn kill_ring_top(&self) -> Option<&str> {
        self.kill_ring.last().map(String::as_str)
    }

    /// Replace the buffer contents and move the cursor to the end.
    pub fn set_text(&mut self, text: &str) {
        // TUI-061 — Pi has TWO functions here and cyrup had collapsed them into one.
        // `setText` (`editor.ts:1010-1021` @v0.83.0) is the PROGRAMMATIC entry point:
        //
        // ```ts
        // this.cancelAutocomplete();
        // this.lastAction = null;
        // this.exitHistoryBrowsing();
        // const normalized = this.normalizeText(text);
        // if (this.getText() !== normalized) this.pushUndoSnapshot();   // makes it undoable
        // this.pastes.clear(); this.pasteCounter = 0;
        // this.setTextInternal(normalized);
        // ```
        //
        // `setTextInternal` (`:1043-1056`) — "Internal setText that doesn't reset history state -
        // used by navigateHistory" — does none of it. Collapsing the two left the paste registry
        // ALIVE across a programmatic buffer replacement (so a subsequently hand-typed
        // `[paste #1 …]` still expanded — TUI-049's surface, narrowed but not closed by that fix)
        // and made the replacement un-undoable.
        self.autocomplete = None;
        self.last_action = LastAction::None;
        self.exit_history();
        if self.text() != text {
            self.push_undo_for(LastAction::None);
        }
        self.pastes.clear();
        self.paste_counter = 0;
        self.set_text_internal(text);
    }

    /// Pi's `setTextInternal` (`editor.ts:1043-1056` @v0.83.0) — "Internal setText that doesn't
    /// reset history state - used by navigateHistory". No autocomplete cancel, no undo snapshot, no
    /// registry reset: the buffer is replaced and the scroll re-anchored, nothing else. TUI-061.
    fn set_text_internal(&mut self, text: &str) {
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.lines.get(self.row).map_or(0, Vec::len);
        // `editor.ts:471`: "Reset scroll - render() will adjust to show cursor".
        self.scroll_offset = 0;
    }

    /// Clear the buffer back to a single empty line and reset transient state.
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.autocomplete = None;
        self.jump = None;
        self.last_action = LastAction::None;
        self.preferred_visual_col = None;
        self.scroll_offset = 0;
        self.pastes.clear();
        // `this.pastes.clear(); this.pasteCounter = 0;` — both halves, on both upstream paths this
        // method stands in for (`submitValue`, `editor.ts:1264-1266`; `setText`, `:1018-1020`).
        // Clearing the map without resetting the counter left paste ids drifting up for the life of
        // the session, so cyrup's `[paste #7 …]` was pi's `[paste #1 …]`.
        self.paste_counter = 0;
        self.exit_history();
    }

    /// The char length of the current line (0 if somehow out of range).
    fn cur_len(&self) -> usize {
        self.lines.get(self.row).map_or(0, Vec::len)
    }

    // ---- visual-line map (wrap-aware vertical motion, spec/tui/03 §4) -----------------------

    /// Record the layout width used to wrap lines for vertical motion (set every render; also a test
    /// seam). `0` is clamped to `1` so wrapping never divides by zero.
    pub fn set_view_width(&mut self, width: usize) {
        self.view_width = width.max(1);
    }

    /// Tell the editor how many rows the host TERMINAL has — pi's `this.tui.terminal.rows`
    /// (`editor.ts:500`). Drives [`term_rows`](Self::term_rows), i.e. the `max(5, floor(rows *
    /// 0.3))` window the editor caps itself at. The app publishes this every `draw`; an embedder
    /// that never calls it gets pi's `?? 24` default.
    pub fn set_terminal_height(&mut self, rows: u16) {
        self.term_rows = rows.max(1);
    }

    /// The editor's own visible-row budget at the current [`term_rows`](Self::term_rows) —
    /// `Math.max(5, Math.floor(terminalRows * 0.3))` (`editor.ts:501`).
    pub fn max_visible_lines(&self) -> u16 {
        crate::app::max_visible_editor_lines(self.term_rows)
    }

    /// Build the wrap-aware visual-line map at the current [`view_width`](Self::view_width)
    /// (`editor.ts:1690` `build_visual_line_map`). Each logical line expands into one or more
    /// [`VisualLine`]s via word-aware wrapping; the result is in reading order and always non-empty
    /// (at least one zero-length visual line for the single empty buffer line).
    pub fn visual_line_map(&self) -> Vec<VisualLine> {
        let width = self.view_width.max(1);
        let mut map = Vec::with_capacity(self.lines.len());
        for (logical, line) in self.lines.iter().enumerate() {
            for (start, len) in word_wrap_line(line, width) {
                map.push(VisualLine { logical, start, len });
            }
        }
        if map.is_empty() {
            map.push(VisualLine { logical: 0, start: 0, len: 0 });
        }
        map
    }

    /// The number of **visual** (wrapped) lines the buffer occupies at `width` columns — the total
    /// [`VisualLine`] count of the wrap map built at an arbitrary width (`editor.ts:1690`
    /// `build_visual_line_map(width).length`, the same primitive vertical motion uses). The app sizes
    /// the editor slot from this so a long/pasted logical line grows the box one row per wrapped
    /// visual line instead of clipping. Independent of [`view_width`](Self::view_width) so height
    /// measurement and render agree when passed the same width. Always `>= 1`.
    pub fn visual_line_count(&self, width: usize) -> usize {
        let width = width.max(1);
        let count: usize = self.lines.iter().map(|line| word_wrap_line(line, width).len()).sum();
        count.max(1)
    }

    /// Map the cursor `(row, col)` to its index in `map` (`editor.ts:1742` `find_current_visual_line`):
    /// the visual line of the cursor's logical row that contains `col`; when `col` sits exactly on a
    /// wrap boundary it belongs to the *start* of the following visual line, and an end-of-line cursor
    /// rides the last visual line of the row.
    fn current_visual_line(&self, map: &[VisualLine]) -> usize {
        let mut fallback = 0;
        for (i, vl) in map.iter().enumerate() {
            if vl.logical != self.row {
                continue;
            }
            fallback = i;
            if self.col >= vl.start && self.col < vl.start + vl.len {
                return i;
            }
        }
        fallback
    }

    /// Vertical Up by one **visual** line, preserving the sticky preferred column (spec/tui/03 §4.2).
    /// At the first visual line the cursor falls through to line-start (history is handled by the
    /// caller before this runs).
    fn move_up_visual(&mut self) {
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        let here = map.get(cur).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let goal = self.preferred_visual_col.unwrap_or(self.col.saturating_sub(here.start));
        self.preferred_visual_col = Some(goal);
        if cur == 0 {
            // First visual line: fall through to line-start (spec/tui/03 §5.1).
            self.col = 0;
            return;
        }
        if let Some(target) = map.get(cur - 1) {
            self.row = target.logical;
            self.col = target.start + goal.min(target.len);
        }
    }

    /// Vertical Down by one **visual** line, preserving the sticky preferred column (spec/tui/03 §4.2).
    /// At the last visual line the cursor falls through to line-end (history is handled by the caller).
    fn move_down_visual(&mut self) {
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        let here = map.get(cur).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let goal = self.preferred_visual_col.unwrap_or(self.col.saturating_sub(here.start));
        self.preferred_visual_col = Some(goal);
        if cur + 1 >= map.len() {
            // Last visual line: fall through to line-end (spec/tui/03 §5.1).
            self.col = self.cur_len();
            return;
        }
        if let Some(target) = map.get(cur + 1) {
            self.row = target.logical;
            self.col = target.start + goal.min(target.len);
        }
    }

    /// Move the caret by one **page** of visual lines (`editor.ts:1857` `pageScroll(direction)`;
    /// `direction` is `-1` for up, `1` for down).
    ///
    /// Upstream:
    ///
    /// ```text
    /// const pageSize = Math.max(5, Math.floor(terminalRows * 0.3));
    /// const target = Math.max(0, Math.min(visualLines.length - 1, current + direction * pageSize));
    /// this.moveToVisualLine(visualLines, current, target);
    /// ```
    ///
    /// The page size is the SAME `max(5, floor(rows * 0.3))` window the editor renders in
    /// ([`max_visible_lines`](Self::max_visible_lines)), and the move goes through the shared sticky
    /// preferred-column machinery, so a page hop keeps the goal column exactly as Up/Down do.
    ///
    /// Unlike [`move_up_visual`](Self::move_up_visual) / [`move_down_visual`](Self::move_down_visual)
    /// there is **no** history recall and no line-start/line-end fall-through at the ends: upstream
    /// clamps the target index and lets `moveToVisualLine` place the caret (`editor.ts:1863`).
    pub fn page_scroll(&mut self, direction: i8) {
        self.last_action = LastAction::None;
        let page = usize::from(self.max_visible_lines());
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        let here = map.get(cur).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let goal = self.preferred_visual_col.unwrap_or(self.col.saturating_sub(here.start));
        self.preferred_visual_col = Some(goal);
        let last = map.len().saturating_sub(1);
        let target = if direction < 0 { cur.saturating_sub(page) } else { (cur + page).min(last) };
        if let Some(t) = map.get(target) {
            self.row = t.logical;
            self.col = t.start + goal.min(t.len);
        }
    }

    /// Whether the buffer occupies more than one **visual** line at the current layout width, i.e.
    /// whether there is anything inside the editor for a page hop to move through.
    ///
    /// The app consults this to decide whether `PageUp`/`PageDown` belongs to the editor (pi's only
    /// binding for those keys, `keybindings.ts:89-90`) or falls through to cyrup's active-region
    /// transcript scroll — see [`crate::app::App::handle_input`].
    pub fn is_multi_visual_line(&self) -> bool {
        self.visual_line_map().len() > 1
    }

    /// Drop the sticky vertical-motion column (called by every non-vertical motion/edit so the next
    /// Up/Down re-seeds the goal column from the live cursor).
    fn reset_preferred_col(&mut self) {
        self.preferred_visual_col = None;
    }

    // ---- large-paste markers (spec/tui/03 §5.5) --------------------------------------------

    /// Handle a (bracketed) paste (`editor.ts:615` `handlePaste`): sanitize, then either collapse a
    /// **large** paste (`> 10` lines or `> 1000` chars) to an atomic `[paste #N …]` marker stored in
    /// [`pastes`](Self::pastes), or insert a small paste verbatim. The marker keeps the buffer compact;
    /// [`expanded_text`](Self::expanded_text) restores the full content on submit.
    pub fn handle_paste(&mut self, raw: &str) {
        let text = sanitize_paste(raw);
        let line_count = text.split('\n').count();
        let char_count = text.chars().count();
        if line_count > 10 || char_count > 1000 {
            // The snapshot is the FIRST thing `handlePaste` does (`editor.ts:1160`), *before* the
            // counter and the registry are touched — so one undo rolls the paste back completely and
            // the next paste re-issues the same id. cyrup pushed it after `pastes.insert` + the
            // increment, which is why paste → undo → paste re-issued `#2` where pi re-issues `#1`
            // (TUI-042's quiet variant).
            self.push_undo_for(LastAction::None);
            self.paste_counter += 1;
            let id = self.paste_counter;
            let marker = if line_count > 1 {
                format!("[paste #{id} +{line_count} lines]")
            } else {
                format!("[paste #{id} {char_count} chars]")
            };
            self.pastes.insert(id, text);
            for c in marker.chars() {
                self.insert_char(c);
            }
            self.last_action = LastAction::None;
            self.reset_preferred_col();
            self.exit_history();
            self.update_autocomplete();
        } else {
            self.insert_str(&text);
        }
    }

    /// The buffer text with every `[paste #N …]` marker expanded back to its stored content
    /// (`expandPasteMarkers`, `editor.ts`). Submission uses this so the model receives the full paste.
    pub fn expanded_text(&self) -> String {
        let text = self.text();
        if self.pastes.is_empty() {
            return text;
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            if let Some((_, content, end)) = self.marker_at(&chars, i) {
                out.push_str(content);
                i = end;
            } else if let Some(c) = chars.get(i) {
                out.push(*c);
                i += 1;
            } else {
                break;
            }
        }
        out
    }

    /// If a `[paste #N …]` marker for a known id starts at `chars[i]`, return its `(id, content, end)`
    /// where `end` is the char index just past the closing `]`. Bounds-checked throughout (no-panic).
    ///
    /// The accepted grammar is `PASTE_MARKER_SINGLE` (`editor.ts:24` @v0.83.0), anchored at `i`:
    ///
    /// ```text
    /// /^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$/
    /// ```
    ///
    /// i.e. the id, then **either** an immediate `]`, **or** one space and exactly one of
    /// `+<digits> lines` / `<digits> chars` before the `]` — the two shapes
    /// [`handle_paste`](Self::handle_paste) produces, plus the bare `[paste #N]` the regex allows.
    /// The previous implementation scanned to the first `]` with the body unconstrained, so a
    /// hand-typed `[paste #1 see the file above]` matched and [`expanded_text`](Self::expanded_text)
    /// silently replaced the user's own words with the stored paste (TUI-049). The id must also be
    /// live in [`pastes`](Self::pastes) — pi's `validIds` gate (`segmentWithMarkers`, `:44`).
    fn marker_at<'a>(&'a self, chars: &[char], i: usize) -> Option<(u32, &'a str, usize)> {
        let (id, _, end) = marker_span_at(chars, i)?;
        let content = self.pastes.get(&id)?;
        Some((id, content.as_str(), end))
    }

    /// Every **valid** marker span on `chars` as `(start, end, id)`, left to right and
    /// non-overlapping — the marker scan `segmentWithMarkers` runs before merging
    /// (`editor.ts:48-57`: `for (const m of text.matchAll(PASTE_MARKER_REGEX)) { if
    /// (!validIds.has(id)) continue; markers.push(…) }`).
    fn marker_spans(&self, chars: &[char]) -> Vec<(usize, usize, u32)> {
        let mut spans = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            match self.marker_at(chars, i) {
                Some((id, _, end)) => {
                    spans.push((i, end, id));
                    i = end;
                }
                None => i += 1,
            }
        }
        spans
    }

    // `marker_covering(col)` — "is `col` inside or on either edge of a marker" — used to be the whole
    // of cyrup's marker atomicity, called from `backspace()` and `delete()` and from nowhere else. It
    // is gone: upstream has no such predicate. Atomicity there is a property of the SEGMENTER
    // (`this.segment(text, "grapheme" | "word")`, `editor.ts:361-363`), which every motion and
    // deletion path already goes through, so the marker is atomic for cursor motion too and the
    // caret can never be parked inside one. See [`marker_grapheme_boundaries`](Self::marker_grapheme_boundaries)
    // and [`word_segments`](Self::word_segments).

    /// Retire the paste a just-backspaced `[paste #N …]` marker owned, then **renumber** — a literal
    /// port of `handleBackspace`'s paste branch (`editor.ts:1293-1315` @v0.83.0):
    ///
    /// ```text
    /// this.pastes.delete(targetId);
    /// this.pasteCounter--;
    /// // Shift registry entries down in ascending id order …
    /// const higherIds = [...this.pastes.keys()].filter((id) => id > targetId).sort((a, b) => a - b);
    /// for (const id of higherIds) { this.pastes.set(id - 1, this.pastes.get(id)!); this.pastes.delete(id); }
    /// // Renumber markers with ids greater than the removed one.
    /// this.state.lines = this.state.lines.map((line) => line.replace(PASTE_MARKER_REGEX, …));
    /// ```
    ///
    /// A `BTreeMap` already iterates ascending, which is what upstream's `.sort()` buys. The text
    /// rewrite runs on the **syntactic** matcher with no `validIds` filter, exactly as upstream's
    /// bare `PASTE_MARKER_REGEX` replace does.
    ///
    /// [CYRUP-DELTA] none — including the hazard: renumbering `#10` → `#9` shortens a line, and
    /// upstream computes the deletion offsets *before* the rewrite and re-reads the line *after* it
    /// (`:1317-1322`), so a two-digit marker earlier on the same line shifts the deletion. That is
    /// upstream's arithmetic and it is reproduced rather than quietly corrected; see the report.
    fn drop_paste(&mut self, target: u32) {
        self.pastes.remove(&target);
        self.paste_counter = self.paste_counter.saturating_sub(1);
        let higher: Vec<u32> = self.pastes.keys().copied().filter(|&id| id > target).collect();
        for id in higher {
            if let Some(content) = self.pastes.remove(&id) {
                self.pastes.insert(id.saturating_sub(1), content);
            }
        }
        for line in &mut self.lines {
            *line = renumber_markers(line, target);
        }
    }

    /// Snapshot the buffer + cursor **+ the paste registry** for undo — pi's `pushUndoSnapshot`
    /// payload `{ state, pastes, pasteCounter }` (`editor.ts:2012-2014` @v0.83.0), deep-copied by
    /// `structuredClone` upstream (`undo-stack.ts:11-13`) and by `Clone` here.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
            pastes: self.pastes.clone(),
            paste_counter: self.paste_counter,
        }
    }

    /// Push an undo snapshot, coalescing consecutive typing into one unit (fish-style,
    /// `editor.ts:1082-1095`): a run of `Type` actions shares a single snapshot.
    fn push_undo_for(&mut self, action: LastAction) {
        let coalesce = action == LastAction::Type && self.last_action == LastAction::Type;
        if !coalesce {
            self.undo.push(self.snapshot());
            // Bound the stack so a long session does not grow unbounded.
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
    }

    /// Push an undo snapshot for a typed character, honoring Pi's fish-style **whitespace boundary**
    /// (`editor.ts:1085-1094`, item #5 "undo-whitespace coalescing"): consecutive word characters
    /// coalesce into one unit, but a whitespace character *always* captures the state before itself
    /// (`isWhitespaceChar(char) || lastAction !== "type-word"`), so a single `Ctrl+-` removes the last
    /// word without swallowing the whole line, and each space is a separate undo step.
    fn push_undo_for_type(&mut self, c: char) {
        if c.is_whitespace() || self.last_action != LastAction::Type {
            self.undo.push(self.snapshot());
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
    }

    /// Restore the most recent undo snapshot (Ctrl+-). No redo (Pi parity). A statement-for-statement
    /// port of `undo()` (`editor.ts:2016-2030` @v0.83.0):
    ///
    /// ```text
    /// this.exitHistoryBrowsing();
    /// const snapshot = this.undoStack.pop();
    /// if (!snapshot) return;
    /// Object.assign(this.state, snapshot.state);
    /// this.pastes = snapshot.pastes;
    /// this.pasteCounter = snapshot.pasteCounter;
    /// this.lastAction = null;
    /// this.preferredVisualCol = null;
    /// ```
    ///
    /// `Object.assign(this.state, …)` restores **both** cursor coordinates (`EditorState` is
    /// `{ lines, cursorLine, cursorCol }`, `:209-213`) — cyrup used to keep the live `self.col` and
    /// merely clamp it, so the caret ended up wherever it happened to be and the next keystroke
    /// edited a position the user never chose (TUI-044). `min(cur_len())` is a bounds guard only.
    /// `last_action` is cleared by the [`EditorAction::Undo`] arm, matching `this.lastAction = null`.
    fn undo(&mut self) {
        self.exit_history();
        if let Some(snap) = self.undo.pop() {
            self.lines = snap.lines;
            self.row = snap.row.min(self.lines.len().saturating_sub(1));
            self.col = snap.col.min(self.cur_len());
            self.pastes = snap.pastes;
            self.paste_counter = snap.paste_counter;
            self.reset_preferred_col();
        }
    }

    // ---- insertion -------------------------------------------------------------------------

    /// Insert a printable character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let col = self.col.min(self.cur_len());
        if let Some(line) = self.lines.get_mut(self.row) {
            line.insert(col, c);
            self.col = col + 1;
        }
        self.preferred_visual_col = None;
    }

    /// Insert a string (e.g. a paste) char by char.
    pub fn insert_str(&mut self, s: &str) {
        self.push_undo_for(LastAction::None);
        for c in s.chars() {
            if c == '\n' {
                self.insert_newline();
            } else {
                self.insert_char(c);
            }
        }
        self.last_action = LastAction::None;
        self.exit_history();
        self.update_autocomplete();
    }

    /// Split the current line at the cursor (newline).
    pub fn insert_newline(&mut self) {
        let col = self.col.min(self.cur_len());
        let tail = match self.lines.get_mut(self.row) {
            Some(line) => line.split_off(col),
            None => Vec::new(),
        };
        let next = self.row + 1;
        self.lines.insert(next, tail);
        self.row = next;
        self.col = 0;
    }

    // ---- deletion --------------------------------------------------------------------------

    /// Backspace: delete the whole grapheme cluster before the cursor (emoji/ZWJ/combining marks
    /// removed as one unit, `editor.ts`), joining lines at column 0.
    pub fn backspace(&mut self) {
        if self.col > 0 {
            // The cluster about to be deleted, marker-aware ([`marker_grapheme_boundaries`]) — pi
            // takes the LAST segment of `line.slice(0, cursorCol)` under `this.segment(…,
            // "grapheme")` (`editor.ts:1287-1290`), which is a whole `[paste #N …]` marker exactly
            // when the caret sits on the marker's closing `]`.
            let start = self.prev_grapheme(self.col);
            // "This contains the id part e.g 4 from [paste #4 +123 lines]" (`editor.ts:1291-1315`):
            // when the deleted cluster IS a marker, drop its registry entry and renumber.
            let deleted_marker = self
                .lines
                .get(self.row)
                .and_then(|line| self.marker_at(line, start))
                .filter(|&(_, _, end)| end == self.col)
                .map(|(id, _, _)| id);
            if let Some(target) = deleted_marker {
                self.drop_paste(target);
            }
            if let Some(line) = self.lines.get_mut(self.row) {
                let end = self.col.min(line.len());
                if start < end {
                    line.drain(start..end);
                }
            }
            self.col = start;
        } else if self.row > 0 && self.row < self.lines.len() {
            let cur = self.lines.remove(self.row);
            let prev_row = self.row - 1;
            if let Some(prev) = self.lines.get_mut(prev_row) {
                let join = prev.len();
                prev.extend(cur);
                self.row = prev_row;
                self.col = join;
            }
        }
    }

    /// Forward-delete: delete the whole grapheme cluster at the cursor (one user-perceived char),
    /// joining the next line at end-of-line.
    ///
    /// The cluster is marker-aware ([`marker_grapheme_boundaries`](Self::marker_grapheme_boundaries)),
    /// so Delete at a marker's `[` removes the whole marker — pi's `handleForwardDelete` takes the
    /// FIRST segment of `line.slice(cursorCol)` under `this.segment(…, "grapheme")`
    /// (`editor.ts:1687-1690`). Note the deliberate asymmetry with [`backspace`](Self::backspace):
    /// upstream's forward-delete has **no** paste branch — it neither drops the registry entry nor
    /// renumbers (`:1674-1706`), so neither does this.
    pub fn delete(&mut self) {
        let len = self.cur_len();
        if self.col < len {
            let end = self.next_grapheme(self.col);
            if let Some(line) = self.lines.get_mut(self.row) {
                let end = end.min(line.len());
                if self.col < end {
                    line.drain(self.col..end);
                }
            }
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            if let Some(line) = self.lines.get_mut(self.row) {
                line.extend(next);
            }
        }
    }

    /// Delete from the word-left boundary to the cursor, feeding the kill ring (Ctrl+W,
    /// `editor.ts:1479`). Coalesces with a preceding kill (prepend).
    fn delete_word_backward(&mut self) {
        let target = self.word_left_target();
        if target == (self.row, self.col) {
            return;
        }
        let killed = self.take_range(target, (self.row, self.col));
        self.push_kill(&killed, false);
        self.row = target.0;
        self.col = target.1;
    }

    /// Delete from the cursor to the word-right boundary, feeding the kill ring (Alt+D).
    fn delete_word_forward(&mut self) {
        let target = self.word_right_target();
        if target == (self.row, self.col) {
            return;
        }
        let killed = self.take_range((self.row, self.col), target);
        self.push_kill(&killed, true);
    }

    /// Delete from line start to the cursor, feeding the kill ring (Ctrl+U).
    fn delete_to_line_start(&mut self) {
        if self.col == 0 {
            return;
        }
        let killed = self.take_range((self.row, 0), (self.row, self.col));
        self.push_kill(&killed, false);
        self.col = 0;
    }

    /// Delete from the cursor to line end, feeding the kill ring (Ctrl+K).
    fn delete_to_line_end(&mut self) {
        let len = self.cur_len();
        if self.col >= len {
            return;
        }
        let killed = self.take_range((self.row, self.col), (self.row, len));
        self.push_kill(&killed, true);
    }

    /// Remove and return the text between two positions, **crossing logical lines** (item #5: cross-
    /// line word/char ops). A same-line range drains within the row; a multi-line range removes the
    /// tail of `start.0`, every whole line strictly between, and the head of `end.0`, then joins the
    /// two boundary rows — so a `Ctrl+W`/`Alt+D` (or `Backspace`/`Delete`) at a line edge deletes into
    /// the neighbouring line and re-joins it (`editor.ts` word/char deletion; `word-navigation.ts`
    /// returns cross-line targets). The removed text carries the `\n`s so it yanks back verbatim.
    fn take_range(&mut self, start: (usize, usize), end: (usize, usize)) -> String {
        // Normalize so `start <= end`.
        let (start, end) =
            if (start.0, start.1) <= (end.0, end.1) { (start, end) } else { (end, start) };
        if start == end {
            return String::new();
        }
        // Same-line: drain within the row.
        if start.0 == end.0 {
            let Some(line) = self.lines.get_mut(start.0) else { return String::new() };
            let lo = start.1.min(line.len());
            let hi = end.1.min(line.len());
            if lo >= hi {
                return String::new();
            }
            return line.drain(lo..hi).collect();
        }
        // Multi-line: guard the boundary rows.
        if start.0 >= self.lines.len() || end.0 >= self.lines.len() {
            return String::new();
        }
        let start_col = start.1.min(self.lines.get(start.0).map_or(0, Vec::len));
        let end_col = end.1.min(self.lines.get(end.0).map_or(0, Vec::len));

        // Collect the removed text (start tail + whole inner rows + end head), `\n`-joined, so it
        // yanks back verbatim.
        let mut killed = String::new();
        if let Some(first) = self.lines.get(start.0) {
            killed.extend(first.iter().skip(start_col));
        }
        for r in (start.0 + 1)..end.0 {
            killed.push('\n');
            if let Some(row) = self.lines.get(r) {
                killed.extend(row.iter());
            }
        }
        killed.push('\n');
        if let Some(last) = self.lines.get(end.0) {
            killed.extend(last.iter().take(end_col));
        }

        // Splice: keep the head of `start.0`, append the tail of `end.0`, drop the rows between.
        let tail: Vec<char> = self
            .lines
            .get(end.0)
            .map(|l| l.iter().skip(end_col).copied().collect())
            .unwrap_or_default();
        if let Some(first) = self.lines.get_mut(start.0) {
            first.truncate(start_col);
            first.extend(tail);
        }
        self.lines.drain((start.0 + 1)..=end.0);
        killed
    }

    /// Push killed text onto the ring, accumulating into the top entry when the previous action was
    /// also a kill (prepend for backward kills, append for forward kills; `kill-ring.ts`).
    fn push_kill(&mut self, text: &str, append: bool) {
        if self.last_action == LastAction::Kill
            && let Some(top) = self.kill_ring.last_mut() {
                if append {
                    top.push_str(text);
                } else {
                    *top = format!("{text}{top}");
                }
                return;
            }
        self.kill_ring.push(text.to_string());
    }

    /// Yank the kill-ring top at the cursor (Ctrl+Y, `editor.ts:1852`).
    fn yank(&mut self) {
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                if c == '\n' {
                    self.insert_newline();
                } else {
                    self.insert_char(c);
                }
            }
        }
    }

    /// Yank-pop: only after a yank with ≥2 ring entries — delete the just-yanked text, rotate the
    /// ring, and insert the new top (Alt+Y, `editor.ts:1867`).
    fn yank_pop(&mut self) {
        if self.last_action != LastAction::Yank || self.kill_ring.len() < 2 {
            return;
        }
        // Delete the previously-yanked text (the current ring top) backward from the cursor.
        if let Some(prev) = self.kill_ring.last().cloned() {
            let n = prev.chars().count();
            for _ in 0..n {
                self.backspace();
            }
        }
        // Rotate: move the top to the front (so a fresh top becomes current).
        if let Some(top) = self.kill_ring.pop() {
            self.kill_ring.insert(0, top);
        }
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                self.insert_char(c);
            }
        }
    }

    // ---- motion ----------------------------------------------------------------------------

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col = self.prev_grapheme(self.col);
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.cur_len();
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.cur_len() {
            self.col = self.next_grapheme(self.col);
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// The grapheme-cluster boundaries of the current line **with every valid paste marker merged
    /// into one cluster** — pi's `this.segment(text, "grapheme")` (`editor.ts:361-363`), the
    /// segmenter `moveCursor` (`:1808-1830`), `handleBackspace` (`:1287-1290`) and
    /// `handleForwardDelete` (`:1687-1690`) all step by. Without the merge the caret can be parked
    /// INSIDE a `[paste #N …]` marker, where the next keystroke silently destroys it (TUI-043's
    /// cursor-motion half).
    fn marker_grapheme_boundaries(&self, line: &[char]) -> Vec<usize> {
        let mut bounds = grapheme_boundaries(line);
        let markers = self.marker_spans(line);
        if !markers.is_empty() {
            bounds.retain(|&b| !markers.iter().any(|&(s, e, _)| b > s && b < e));
        }
        bounds
    }

    /// The previous grapheme-cluster boundary strictly left of char-column `col` on the current line
    /// (emoji/ZWJ/combining marks — and whole paste markers — step as one unit). `0` if none.
    fn prev_grapheme(&self, col: usize) -> usize {
        let Some(line) = self.lines.get(self.row) else { return col.saturating_sub(1) };
        self.marker_grapheme_boundaries(line).into_iter().rfind(|&b| b < col).unwrap_or(0)
    }

    /// The next grapheme-cluster boundary strictly right of char-column `col` on the current line.
    /// Clamps to the line length when `col` is already at/after the last cluster.
    fn next_grapheme(&self, col: usize) -> usize {
        let Some(line) = self.lines.get(self.row) else { return col + 1 };
        let len = line.len();
        self.marker_grapheme_boundaries(line).into_iter().find(|&b| b > col).unwrap_or(len)
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.cur_len();
    }

    /// Word-granularity segments of `text`, with every **valid** paste marker merged into one atomic
    /// segment — pi's `this.segment(text, "word")` (`editor.ts:361-363`), i.e. `segmentWithMarkers`
    /// (`:37-90`) over `Intl.Segmenter(undefined, { granularity: "word" })` (`utils.ts:5`).
    ///
    /// [CYRUP-DELTA] The base segmenter is `unicode_segmentation`'s UAX#29 word-boundary iterator
    /// rather than ICU's. They agree on Latin/Cyrillic/Greek prose, identifiers, `foo.bar`, `don't`
    /// and `3.14`; they differ on **unspaced scripts**, where ICU adds a dictionary/LSTM pass that
    /// UAX#29 alone has no data for — `你好世界` is two segments to ICU and four to UAX#29. Closing
    /// that needs an ICU-class word segmenter (`icu_segmenter` + its CJK/Thai data), which is a new
    /// workspace dependency and not this change's to take. See TUI-048.
    fn word_segments(&self, text: &[char]) -> Vec<WordSeg> {
        let markers = self.marker_spans(text);
        let joined: String = text.iter().collect();
        let mut out: Vec<WordSeg> = Vec::new();
        let mut col = 0usize;
        let mut mi = 0usize;
        for seg in joined.split_word_bounds() {
            let len = seg.chars().count();
            let start = col;
            col += len;
            // "Skip past markers that are entirely before this segment" (`editor.ts:67-69`).
            while markers.get(mi).is_some_and(|&(_, end, _)| end <= start) {
                mi += 1;
            }
            match markers.get(mi) {
                // "This segment falls inside a marker" (`:74`): emit the merged segment once, at the
                // marker's first base segment, and skip the rest (`:76-86`).
                Some(&(ms, me, _)) if start >= ms && start < me => {
                    if start == ms {
                        out.push(WordSeg {
                            start: ms,
                            len: me.saturating_sub(ms),
                            word_like: false,
                            atomic: true,
                        });
                    }
                }
                _ => out.push(WordSeg {
                    start,
                    len,
                    word_like: seg.chars().any(char::is_alphanumeric),
                    atomic: false,
                }),
            }
        }
        out
    }

    /// Whether `seg` is whitespace — pi's `isWhitespaceChar(segment)` = `/\s/.test(segment)`
    /// (`utils.ts:826-829`), which is *contains* whitespace, hence `any`.
    fn seg_is_whitespace(text: &[char], seg: &WordSeg) -> bool {
        text.get(seg.start..seg.start.saturating_add(seg.len))
            .is_some_and(|s| s.iter().any(|c| c.is_whitespace()))
    }

    /// The word-left target `(row, col)` — a statement-for-statement port of `findWordBackward`
    /// (`pi/packages/tui/src/word-navigation.ts:22-68` @v0.83.0) as pi calls it from
    /// `moveWordBackwards` (`editor.ts:1869-1889`), i.e. with
    /// `{ segment: (t) => this.segment(t, "word"), isAtomicSegment: isPasteMarker }`.
    ///
    /// Three branches after the whitespace skip: **one atomic segment** whole (`:44-46` — a
    /// `[paste #N …]` marker is never entered, which is what makes Ctrl+W delete the marker instead
    /// of chewing its closing `]`, TUI-043), **one word-like segment** truncated at its last
    /// `PUNCTUATION_REGEX` match (`:47-57`), or **a whole punctuation run** (`:58-66`).
    /// At col 0 step to the previous line's end (`editor.ts:1874-1881`).
    fn word_left_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let cursor = self.col.min(line.len());
        if cursor == 0 {
            if self.row > 0 {
                let prev_len = self.lines.get(self.row - 1).map_or(0, Vec::len);
                return (self.row - 1, prev_len);
            }
            return (self.row, 0);
        }
        // `const textBeforeCursor = text.slice(0, cursor)` (`:25`) — segmenting only the PREFIX is
        // why a marker the cursor sits inside is not atomic: it is not whole in this slice.
        let Some(before) = line.get(..cursor) else { return (self.row, cursor) };
        let mut segs = self.word_segments(before);
        let mut new_cursor = cursor;

        // "Skip trailing whitespace" (`:31-38`).
        while let Some(last) = segs.last() {
            if last.atomic || !Self::seg_is_whitespace(before, last) {
                break;
            }
            new_cursor = new_cursor.saturating_sub(last.len);
            segs.pop();
        }
        // `if (segments.length === 0) return newCursor` (`:40`).
        let Some(&last) = segs.last() else { return (self.row, new_cursor) };

        if last.atomic {
            // "Skip one atomic segment" (`:44-46`).
            new_cursor = new_cursor.saturating_sub(last.len);
        } else if last.word_like {
            // "Skip inside one word-like segment, preserving ASCII punctuation boundaries"
            // (`:47-57`): back up to just after the LAST punctuation character in the segment.
            let seg = before.get(last.start..last.start.saturating_add(last.len)).unwrap_or(&[]);
            match seg.iter().rposition(|&c| is_punctuation(c)) {
                None => new_cursor = new_cursor.saturating_sub(last.len),
                Some(idx) => {
                    new_cursor = new_cursor.saturating_sub(last.len.saturating_sub(idx + 1));
                }
            }
        } else {
            // "Skip non-word non-whitespace run (punctuation)" (`:58-66`).
            while let Some(last) = segs.last() {
                if last.atomic || last.word_like || Self::seg_is_whitespace(before, last) {
                    break;
                }
                new_cursor = new_cursor.saturating_sub(last.len);
                segs.pop();
            }
        }
        (self.row, new_cursor)
    }

    /// The word-right target — the mirror port of `findWordForward` (`word-navigation.ts:76-114`),
    /// called as `moveWordForwards` does (`editor.ts:2064-2083`). Same three branches, with the
    /// atomic skip at `:97-99` and the word-like branch taking the FIRST punctuation match (`:102`).
    fn word_right_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let len = line.len();
        let cursor = self.col.min(len);
        if cursor >= len {
            if self.row + 1 < self.lines.len() {
                return (self.row + 1, 0);
            }
            return (self.row, len);
        }
        // `const textAfterCursor = text.slice(cursor)` (`:79`).
        let Some(after) = line.get(cursor..) else { return (self.row, cursor) };
        let segs = self.word_segments(after);
        let mut idx = 0usize;
        let mut new_cursor = cursor;

        // "Skip leading whitespace" (`:88-93`).
        while let Some(seg) = segs.get(idx) {
            if seg.atomic || !Self::seg_is_whitespace(after, seg) {
                break;
            }
            new_cursor = new_cursor.saturating_add(seg.len);
            idx += 1;
        }
        // `if (next.done) return newCursor` (`:95`).
        let Some(&next) = segs.get(idx) else { return (self.row, new_cursor) };

        if next.atomic {
            new_cursor = new_cursor.saturating_add(next.len);
        } else if next.word_like {
            let seg = after.get(next.start..next.start.saturating_add(next.len)).unwrap_or(&[]);
            let step = seg.iter().position(|&c| is_punctuation(c)).unwrap_or(next.len);
            new_cursor = new_cursor.saturating_add(step);
        } else {
            while let Some(seg) = segs.get(idx) {
                if seg.atomic || seg.word_like || Self::seg_is_whitespace(after, seg) {
                    break;
                }
                new_cursor = new_cursor.saturating_add(seg.len);
                idx += 1;
            }
        }
        (self.row, new_cursor)
    }

    fn move_word_left(&mut self) {
        let (r, c) = self.word_left_target();
        self.row = r;
        self.col = c;
    }

    fn move_word_right(&mut self) {
        let (r, c) = self.word_right_target();
        self.row = r;
        self.col = c;
    }

    // ---- char-jump -------------------------------------------------------------------------

    /// Jump the cursor to the next/previous occurrence of `target` on the current line, skipping the
    /// current position (case-sensitive, `editor.ts:1990-2018`).
    fn jump_to(&mut self, dir: JumpDir, target: char) {
        let Some(line) = self.lines.get(self.row) else { return };
        match dir {
            JumpDir::Forward => {
                if let Some(off) = line.iter().enumerate().skip(self.col + 1).find_map(|(i, &c)| {
                    (c == target).then_some(i)
                }) {
                    self.col = off;
                }
            }
            JumpDir::Backward => {
                if let Some(off) = (0..self.col)
                    .rev()
                    .find(|&i| line.get(i) == Some(&target))
                {
                    self.col = off;
                }
            }
        }
    }

    // ---- history ---------------------------------------------------------------------------

    /// Seed the prompt history with an already-submitted line — Pi's `editor.addToHistory?.(text)`
    /// on the `populateHistory` replay path (interactive-mode.ts:3387). Same skip rules as a live
    /// submission; call in chronological order so the newest replayed prompt ends up first.
    pub fn push_history(&mut self, text: &str) {
        self.add_to_history(text);
    }

    /// Add a raw submitted line to history (skip blank + consecutive-dup, `editor.ts:381-391`).
    fn add_to_history(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.history.front().map(String::as_str) == Some(text) {
            return;
        }
        self.history.push_front(text.to_string());
        while self.history.len() > HISTORY_CAP {
            self.history.pop_back();
        }
    }

    /// Whether Up should enter / continue history browsing (`editor.ts:809-822`): at row 0 and the
    /// buffer is empty, already browsing, or the cursor is at col 0.
    fn history_up_eligible(&self) -> bool {
        self.row == 0 && (self.is_empty() || self.history_index >= 0 || self.col == 0)
    }

    /// Older history entry (Up). On first entry, snapshot the draft.
    fn history_older(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index < 0 {
            // `navigateHistory` (`editor.ts:435-438` @v0.83.0) pushes an undo snapshot *and* clones
            // the draft when browsing is first entered: `this.pushUndoSnapshot(); this.historyDraft =
            // structuredClone(this.state);`. cyrup saved the draft but never pushed the snapshot, so
            // Ctrl+- could not undo "I browsed history away from what I was typing".
            self.push_undo_for(LastAction::None);
            self.history_draft = Some(self.snapshot());
        }
        let next = (self.history_index + 1).min(self.history.len() as isize - 1);
        self.history_index = next;
        if let Some(entry) = self.history.get(next as usize).cloned() {
            // TUI-061 — `navigateHistory` uses `setTextInternal` (`editor.ts:1043-1056`), never
            // `setText`: browsing must not cancel the autocomplete, push a snapshot per step, or
            // clear the paste registry the draft is about to be restored with.
            self.set_text_internal(&entry);
            self.row = 0;
            self.col = 0; // cursor at start on Up (setTextInternal placement)
        }
    }

    /// Newer history entry (Down). Past the newest restores the saved draft.
    fn history_newer(&mut self) {
        if self.history_index < 0 {
            return;
        }
        self.history_index -= 1;
        if self.history_index < 0 {
            // Restore the draft (`editor.ts:442-452`: `this.state = draft` + `preferredVisualCol =
            // null` + `scrollOffset = 0`, else `setTextInternal("")`). The draft reuses [`Snapshot`],
            // so it now carries the paste registry too: pi's `historyDraft` is a bare `EditorState`
            // (`:319`) only because nothing upstream mutates `pastes` while browsing — and nothing
            // here does either (every edit path calls `exit_history`, which drops the draft), so
            // restoring both fields is pi's outcome with the invariant made explicit rather than
            // assumed.
            if let Some(draft) = self.history_draft.take() {
                self.lines = draft.lines;
                self.row = draft.row.min(self.lines.len().saturating_sub(1));
                self.col = draft.col.min(self.cur_len());
                self.pastes = draft.pastes;
                self.paste_counter = draft.paste_counter;
                self.reset_preferred_col();
                self.scroll_offset = 0;
            } else {
                // `setTextInternal("")` (`:451`), NOT `clear()`: upstream's fallback empties the
                // buffer and leaves the paste registry, kill ring and popup state alone.
                self.set_text_internal("");
            }
        } else if let Some(entry) = self.history.get(self.history_index as usize).cloned() {
            // TUI-061 — the internal form again (`editor.ts:1043-1056`), same reason as the Up path.
            self.set_text_internal(&entry);
            self.move_end();
        }
    }

    /// Stop browsing history (any edit/newline/submit, `editor.ts`).
    fn exit_history(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
    }

    // ---- autocomplete ----------------------------------------------------------------------

    /// The buffer lines as `String`s, for the autocomplete engine.
    fn lines_as_strings(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Recompute the popup after an edit: auto-open for slash **and** `@`-mention context, otherwise
    /// update an already-open popup or close it (spec/tui/04 §5 — bare path does not auto-pop without
    /// Tab; `@`-mention auto-pops on `@`, `autocomplete.ts:101`).
    fn update_autocomplete(&mut self) {
        let was_open = self.autocomplete.is_some();
        // `@`-mention search auto-pops the moment an `@` token forms (whole-tree fuzzy file search).
        if let Some(ac) = self.compute_mention() {
            self.open_popup(ac);
            return;
        }
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            false,
            &self.cwd,
        );
        match computed {
            Some(ac) if ac.context == CompletionContext::Slash || was_open => self.open_popup(ac),
            _ => self.autocomplete = None,
        }
    }

    /// The text left of the cursor on the current line (the autocomplete context window).
    fn before_cursor(&self) -> String {
        self.lines.get(self.row).map_or(String::new(), |line| line.iter().take(self.col).collect())
    }

    /// Compute the `@`-mention popup for the current cursor, lazily enumerating the tree on first use
    /// (`autocomplete.ts:719-772`). `None` when the trailing token is not an `@`-mention or nothing
    /// matches.
    fn compute_mention(&mut self) -> Option<Autocomplete> {
        let before = self.before_cursor();
        crate::autocomplete::mention_query(&before)?;
        if self.mention_files.is_none() {
            self.mention_files = Some(crate::autocomplete::list_files(&self.cwd, 2000));
        }
        let files = self.mention_files.as_deref().unwrap_or(&[]);
        crate::autocomplete::mention_autocomplete(&before, files)
    }

    /// Trigger completion explicitly (Tab with no popup): force path completion, or slash completion
    /// while typing a `/name` (spec/tui/04 §5). A single forced match auto-applies (§3.7 item 10).
    fn trigger_completion(&mut self) -> EditorOutcome {
        // `@`-mention takes precedence on an explicit Tab too (whole-tree fuzzy file search).
        if let Some(ac) = self.compute_mention() {
            self.open_popup(ac);
            return EditorOutcome::Edited;
        }
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            true,
            &self.cwd,
        );
        match computed {
            Some(ac) if ac.list.len() == 1 && ac.context == CompletionContext::Path => {
                self.open_popup(ac);
                self.accept_completion();
                EditorOutcome::Edited
            }
            Some(ac) => {
                self.open_popup(ac);
                EditorOutcome::Edited
            }
            None => EditorOutcome::Ignored,
        }
    }

    /// Apply the selected popup item to the buffer (Tab/Enter accept). Leaves the popup state to the
    /// caller (Tab keeps editing + recomputes; Enter on a slash item submits).
    fn accept_completion(&mut self) {
        let Some(ac) = self.autocomplete.as_ref() else { return };
        if let Some(applied) = ac.apply(&self.lines_as_strings(), self.row, self.col) {
            self.lines = applied.lines.iter().map(|s| s.chars().collect()).collect();
            if self.lines.is_empty() {
                self.lines.push(Vec::new());
            }
            self.row = applied.cursor_line.min(self.lines.len().saturating_sub(1));
            self.col = applied.cursor_col.min(self.cur_len());
        }
    }

    // ---- key handling ----------------------------------------------------------------------

    /// Feed a key. Resolves an [`EditorAction`] via the keymap (R-10-018); printable chars insert.
    /// While a popup is open, navigation/accept/cancel route first (spec/tui/04 §5).
    pub fn handle_key(&mut self, ev: &KeyEvent) -> EditorOutcome {
        // 1. Char-jump mode consumes the next printable char (or cancels).
        if let Some(dir) = self.jump.take() {
            if let KeyCode::Char(c) = ev.code
                && !ev.modifiers.contains(KeyModifiers::CONTROL) {
                    self.jump_to(dir, c);
                    return EditorOutcome::Edited;
                }
            return EditorOutcome::Edited; // any other key cancels jump
        }

        // 2. Popup-open routing (before normal editing).
        if self.autocomplete.is_some()
            && let Some(outcome) = self.handle_popup_key(ev) {
                return outcome;
            }

        // 3. Resolve a bound editor action.
        if let Some(action) = self.keymap.action_for(ev) {
            return self.apply_editor_action(action);
        }

        // 4. Printable insert (no Ctrl/Super; Alt+char already routed via keymap or ignored).
        if let KeyCode::Char(c) = ev.code
            && !ev.modifiers.contains(KeyModifiers::CONTROL)
                && !ev.modifiers.contains(KeyModifiers::SUPER)
                && !ev.modifiers.contains(KeyModifiers::ALT)
            {
                self.push_undo_for_type(c);
                self.insert_char(c);
                self.last_action = LastAction::Type;
                self.exit_history();
                self.update_autocomplete();
                return EditorOutcome::Edited;
            }
        EditorOutcome::Ignored
    }

    /// Route a key while the popup is open through the configurable [`AutocompleteKeymap`] (item #6 —
    /// the nav/accept/cancel keys are no longer hardcoded). Returns `Some` if consumed; `None` to
    /// fall through to normal editing.
    fn handle_popup_key(&mut self, ev: &KeyEvent) -> Option<EditorOutcome> {
        use crate::keymap::AutocompleteAction as A;
        match self.autocomplete_keymap.action_for(ev)? {
            A::Cancel => {
                self.autocomplete = None;
                Some(EditorOutcome::Edited)
            }
            A::Previous => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_up();
                }
                Some(EditorOutcome::Edited)
            }
            A::Next => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_down();
                }
                Some(EditorOutcome::Edited)
            }
            A::Accept => {
                // Accept, keep editing (no submit), then recompute (may close if out of context).
                self.accept_completion();
                self.update_autocomplete();
                Some(EditorOutcome::Edited)
            }
            A::AcceptSubmit => {
                let is_slash = self
                    .autocomplete
                    .as_ref()
                    .is_some_and(|ac| ac.context == CompletionContext::Slash);
                self.accept_completion();
                self.autocomplete = None;
                if is_slash {
                    // Accepting a slash item with Enter submits (spec/tui/04 §5, edge 15). The accept
                    // appended a trailing space; submit trims it (Pi trims in the submit handler).
                    let text = self.text().trim().to_string();
                    self.add_to_history(&text);
                    self.clear();
                    self.undo.clear(); // `this.undoStack.clear()` (`editor.ts:1268`)
                    Some(EditorOutcome::Submit(text))
                } else {
                    self.update_autocomplete();
                    Some(EditorOutcome::Edited)
                }
            }
        }
    }

    /// Dispatch a resolved editor action.
    fn apply_editor_action(&mut self, action: EditorAction) -> EditorOutcome {
        use EditorAction as E;
        // Any non-vertical action re-seeds the sticky goal column on the next Up/Down (spec/tui/03 §4.2).
        // `PageUp`/`PageDown` ARE vertical motion upstream — `pageScroll` shares `moveToVisualLine`
        // (and therefore `preferredVisualCol`) with `moveCursor` (`editor.ts:1373,1863`).
        if !matches!(action, E::CursorUp | E::CursorDown | E::PageUp | E::PageDown) {
            self.reset_preferred_col();
        }
        match action {
            E::CursorLeft => {
                self.move_left();
                self.last_action = LastAction::None;
                EditorOutcome::Edited
            }
            E::CursorRight => {
                self.move_right();
                self.last_action = LastAction::None;
                EditorOutcome::Edited
            }
            E::CursorUp => {
                // Every motion clears `lastAction` upstream — `moveCursor` (`editor.ts:1791`),
                // `navigateHistory` (`:430`), `moveWordBackwards`/`moveWordForwards` (`:1870`/`:2065`),
                // `moveToLineStart`/`moveToLineEnd`. cyrup cleared it on Left/Right only, so a kill
                // survived a vertical/word/line motion and the NEXT kill accumulated into the same
                // ring entry instead of pushing a new one (and a stale `Yank` still armed Alt+Y).
                self.last_action = LastAction::None;
                // History recall only fires on the first visual line (`history_up_eligible` already
                // requires `row == 0` + empty/browsing/col-0, which is always the first visual line).
                if self.history_up_eligible() {
                    self.history_older();
                } else {
                    self.move_up_visual();
                }
                EditorOutcome::Edited
            }
            E::CursorDown => {
                self.last_action = LastAction::None; // `editor.ts:1791` / `:430`
                if self.history_index >= 0 {
                    self.history_newer();
                } else {
                    self.move_down_visual();
                }
                EditorOutcome::Edited
            }
            // TUI-035 — `tui.editor.historyPrevious` / `historyNext`
            // (`tui/src/components/editor.ts:766-777` @v0.84.1). Upstream's comment is "Dedicated
            // history actions always browse entries instead of moving the cursor", and the two arms
            // sit AHEAD of the cursor-movement block: they cancel the autocomplete and call
            // `navigateHistory(∓1)` UNCONDITIONALLY, with none of the buffer-edge gating Up/Down
            // carry. Default `defaultKeys: []` (`keybindings.ts:68-75`), so nothing is bound until
            // the user says so — which is the point: they exist so Up/Down can be made pure caret
            // motion while history moves to, say, ctrl+p/ctrl+n.
            E::HistoryPrevious => {
                self.autocomplete = None;
                self.last_action = LastAction::None; // `navigateHistory` (`editor.ts:430`)
                self.history_older();
                EditorOutcome::Edited
            }
            E::HistoryNext => {
                self.autocomplete = None;
                self.last_action = LastAction::None;
                self.history_newer();
                EditorOutcome::Edited
            }
            // `tui.editor.pageUp` / `tui.editor.pageDown` (`editor.ts:855-862`): page the CARET
            // through the buffer. No history recall on either end (upstream's `pageScroll` never
            // touches `historyIndex`).
            E::PageUp => {
                self.page_scroll(-1);
                EditorOutcome::Edited
            }
            E::PageDown => {
                self.page_scroll(1);
                EditorOutcome::Edited
            }
            E::CursorWordLeft => {
                self.last_action = LastAction::None; // `moveWordBackwards`, `editor.ts:1870`
                self.move_word_left();
                EditorOutcome::Edited
            }
            E::CursorWordRight => {
                self.last_action = LastAction::None; // `moveWordForwards`, `editor.ts:2065`
                self.move_word_right();
                EditorOutcome::Edited
            }
            E::CursorLineStart => {
                self.last_action = LastAction::None; // `moveToLineStart`, `editor.ts:1783`
                self.move_home();
                EditorOutcome::Edited
            }
            E::CursorLineEnd => {
                self.last_action = LastAction::None; // `moveToLineEnd`, `editor.ts:1787`
                self.move_end();
                EditorOutcome::Edited
            }
            E::DeleteCharBackward => {
                self.push_undo_for(LastAction::None);
                self.backspace();
                self.last_action = LastAction::None;
                self.exit_history();
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteCharForward => {
                self.push_undo_for(LastAction::None);
                self.delete();
                self.last_action = LastAction::None;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteWordBackward => {
                self.push_undo_for(LastAction::Kill);
                self.delete_word_backward();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteWordForward => {
                self.push_undo_for(LastAction::Kill);
                self.delete_word_forward();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteToLineStart => {
                self.push_undo_for(LastAction::Kill);
                self.delete_to_line_start();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteToLineEnd => {
                self.push_undo_for(LastAction::Kill);
                self.delete_to_line_end();
                self.last_action = LastAction::Kill;
                EditorOutcome::Edited
            }
            E::Yank => {
                self.push_undo_for(LastAction::None);
                self.yank();
                self.last_action = LastAction::Yank;
                self.exit_history();
                EditorOutcome::Edited
            }
            E::YankPop => {
                self.yank_pop();
                self.last_action = LastAction::Yank;
                EditorOutcome::Edited
            }
            E::Undo => {
                self.undo();
                self.last_action = LastAction::None;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::NewLine => {
                self.push_undo_for(LastAction::None);
                self.insert_newline();
                self.last_action = LastAction::None;
                self.exit_history();
                EditorOutcome::Edited
            }
            E::Submit => {
                // Backslash-Enter → soft newline (Pi `editor.ts:796-802`, spec/tui/03 §5.7): a
                // workaround for terminals without Shift+Enter. If the char immediately before the
                // cursor is a literal backslash, delete it and insert a newline INSTEAD of submitting
                // (Pi `handleBackspace()` + `addNewLine()`), so `foo\<Enter>` breaks the line.
                if self.col > 0
                    && self.lines.get(self.row).and_then(|l| l.get(self.col - 1)) == Some(&'\\')
                {
                    self.push_undo_for(LastAction::None);
                    self.backspace();
                    self.insert_newline();
                    self.last_action = LastAction::None;
                    self.exit_history();
                    self.update_autocomplete();
                    return EditorOutcome::Edited;
                }
                // Expand large-paste markers back to their full content before the agent sees the text
                // (`expandPasteMarkers`, spec/tui/03 §5.5).
                let text = self.expanded_text();
                if text.trim().is_empty() {
                    return EditorOutcome::Edited;
                }
                self.add_to_history(&text);
                self.clear();
                // `submitValue` empties the undo stack with the buffer (`editor.ts:1268`), so
                // Ctrl+- after a send cannot resurrect the prompt that was just submitted.
                self.undo.clear();
                EditorOutcome::Submit(text)
            }
            E::Tab => self.trigger_completion(),
            E::JumpForward => {
                self.jump = Some(JumpDir::Forward);
                EditorOutcome::Edited
            }
            E::JumpBackward => {
                self.jump = Some(JumpDir::Backward);
                EditorOutcome::Edited
            }
        }
    }

    /// The hardware-cursor position inside `area` (for IME placement, R-10-009), if focused. Accounts
    /// for the top border rule drawn by [`Component::render`]; the editor has no side borders.
    pub fn cursor_in(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.focused {
            return None;
        }
        // Map the logical caret `(row, col)` to its VISUAL `(vrow, vcol)` via the wrap map (built at
        // the render `view_width`) so the hardware cursor lands on the wrapped row/column, matching
        // the reverse-video soft-cursor cell drawn in `render` (Pi `editor.ts:545-551`).
        let map = self.visual_line_map();
        let vi = self.current_visual_line(&map);
        let vl = map.get(vi).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let vcol = self.col.saturating_sub(vl.start);
        // The caret's COLUMN is the visible width of the text before it, not its char count. Pi never
        // does this arithmetic — it emits a zero-width `CURSOR_MARKER` *inside* the row string
        // (`editor.ts:550`) and lets the terminal advance the cursor by the real cell widths — so a
        // char-count offset is a cyrup-only bug: one emoji ahead of the caret and the hardware cursor
        // (and hence the IME candidate window) lands a column left of the reverse-video cell.
        let before_width: usize = self
            .lines
            .get(vl.logical)
            .map(|l| {
                let s: String = l.iter().skip(vl.start).take(vcol).collect();
                Span::raw(s).width()
            })
            .unwrap_or(0);
        // The text rows start `editorPaddingX` columns in (`Padding::horizontal` on the render
        // block), so the caret must too — Pi prefixes the same `leftPadding` (`editor.ts:522`).
        let x = area
            .x
            .saturating_add(self.effective_padding(area.width))
            .saturating_add(before_width.min(u16::MAX as usize) as u16);
        // The caret rides the SCROLLED window: row `vi` is drawn at `vi - scrollOffset` inside the
        // rules (`editor.ts:519` slices `layoutLines` from `scrollOffset`).
        let y = area
            .y
            .saturating_add(1)
            .saturating_add(vi.saturating_sub(self.scroll_offset).min(u16::MAX as usize) as u16);
        let max_x = area.x.saturating_add(area.width).saturating_sub(1);
        let max_y = area.y.saturating_add(area.height).saturating_sub(1);
        Some((x.min(max_x), y.min(max_y)))
    }
}

/// The ASCII punctuation that sub-divides a word-like segment — a literal port of
/// `PUNCTUATION_REGEX` (`pi/packages/tui/src/utils.ts:821` @v0.83.0):
///
/// ```text
/// /[(){}[\]<>.,;:'"!?+\-=*/\\|&%^$#@~`]/
/// ```
///
/// Deliberately **not** the complement of an `is_alphanumeric() || '_'` word-char test — the two are
/// different sets (that test rejects every non-alphanumeric; this one names 31 specific ASCII
/// characters), and word navigation must use pi's. The old class-run word motion used the former and
/// was replaced wholesale (TUI-043 / TUI-048).
fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | '<'
            | '>'
            | '.'
            | ','
            | ';'
            | ':'
            | '\''
            | '"'
            | '!'
            | '?'
            | '+'
            | '-'
            | '='
            | '*'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '%'
            | '^'
            | '$'
            | '#'
            | '@'
            | '~'
            | '`'
    )
}

/// Read a run of ASCII digits starting at `from`, returning `(value, index just past the run)` — or
/// `None` when there is no digit there (`\d+` in `PASTE_MARKER_SINGLE`).
fn read_digits(chars: &[char], from: usize) -> Option<(u32, usize)> {
    let mut j = from;
    let mut value: u32 = 0;
    let mut count = 0usize;
    while let Some(&c) = chars.get(j).filter(|c| c.is_ascii_digit()) {
        value = value.saturating_mul(10).saturating_add(c.to_digit(10).unwrap_or(0));
        j += 1;
        count += 1;
    }
    (count > 0).then_some((value, j))
}

/// Match `PASTE_MARKER_SINGLE` (`editor.ts:24` @v0.83.0) anchored at `chars[i]`, **syntactically** —
/// without consulting the paste registry. Returns `(id, index just past the id digits, index just
/// past the closing `]`)`.
///
/// ```text
/// /^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$/
/// ```
///
/// The registry-gated form is [`InputEditor::marker_at`] (pi's `validIds` filter,
/// `segmentWithMarkers` `:44`). The ungated form exists because pi's marker RENUMBERING replaces on
/// the bare `PASTE_MARKER_REGEX` with no id filter (`editor.ts:1308-1314`).
///
/// `isPasteMarker`'s extra `segment.length >= 10` guard (`:28`) needs no counterpart: the shortest
/// string this grammar accepts is `[paste #1]`, which is exactly 10 characters.
fn marker_span_at(chars: &[char], i: usize) -> Option<(u32, usize, usize)> {
    const PREFIX: [char; 8] = ['[', 'p', 'a', 's', 't', 'e', ' ', '#'];
    for (k, pc) in PREFIX.iter().enumerate() {
        if chars.get(i + k) != Some(pc) {
            return None;
        }
    }
    let (id, digits_end) = read_digits(chars, i + PREFIX.len())?;
    // `( (\+\d+ lines|\d+ chars))?` then `\]`.
    if chars.get(digits_end) == Some(&']') {
        return Some((id, digits_end, digits_end + 1));
    }
    if chars.get(digits_end) != Some(&' ') {
        return None;
    }
    let mut j = digits_end + 1;
    let plus = chars.get(j) == Some(&'+');
    if plus {
        j += 1;
    }
    let (_, after) = read_digits(chars, j)?;
    j = after;
    let tail: &[char] =
        if plus { &[' ', 'l', 'i', 'n', 'e', 's', ']'] } else { &[' ', 'c', 'h', 'a', 'r', 's', ']'] };
    for (n, tc) in tail.iter().enumerate() {
        if chars.get(j + n) != Some(tc) {
            return None;
        }
    }
    Some((id, digits_end, j + tail.len()))
}

/// Rewrite every syntactic `[paste #x …]` marker on `line` with `x > target` as `x - 1`, keeping its
/// suffix — the `line.replace(PASTE_MARKER_REGEX, …)` of `handleBackspace` (`editor.ts:1308-1314`):
///
/// ```text
/// (fullMatch, idGroup, suffixGroup) => { const x = Number(idGroup); if (x <= targetId) return fullMatch;
///                                        return `[paste #${x - 1}${suffixGroup}]`; }
/// ```
fn renumber_markers(line: &[char], target: u32) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(line.len());
    let mut i = 0usize;
    while i < line.len() {
        match marker_span_at(line, i) {
            Some((id, digits_end, end)) => {
                if id > target {
                    out.extend(format!("[paste #{}", id.saturating_sub(1)).chars());
                    out.extend(line.get(digits_end..end).unwrap_or(&[]).iter().copied());
                } else {
                    out.extend(line.get(i..end).unwrap_or(&[]).iter().copied());
                }
                i = end;
            }
            None => {
                if let Some(&c) = line.get(i) {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    out
}

/// One segment of a line for word navigation — pi's `Intl.SegmentData` after `segmentWithMarkers`
/// has merged the paste markers (`editor.ts:37-90`). `start`/`len` are **char columns** into the
/// slice the segments were built from.
#[derive(Clone, Copy, Debug)]
struct WordSeg {
    start: usize,
    len: usize,
    /// `Intl.SegmentData.isWordLike`.
    ///
    /// [CYRUP-DELTA] ICU marks a segment word-like when it is made of letters, digits, kana or
    /// ideographs; `unicode-segmentation` (UAX#29, the same algorithm without ICU's flag) exposes no
    /// such bit, so it is recomputed as "contains an alphanumeric character". The two agree on every
    /// segment UAX#29 can produce: a word-bound segment is either a run of letters/digits (with
    /// MidLetter/MidNumLet joiners), a run of punctuation/symbols, or whitespace.
    word_like: bool,
    /// `isAtomicSegment(segment)` — a whole `[paste #N …]` marker (`isPasteMarker`, `editor.ts:27`).
    atomic: bool,
}

/// The **display width** of `s` in terminal cells — Pi's `visibleWidth` (`utils.ts:240-...`), which
/// is what every wrap and caret computation upstream measures with. `Span::width()` is ratatui's
/// `unicode_width` sum, the same primitive `transcript.rs` already uses for `Box.applyBg`.
///
/// Not `chars().count()`: a CJK ideograph is one `char` and TWO columns, a combining mark is one
/// `char` and ZERO, so a char count is neither an upper nor a lower bound on the cells a string
/// occupies.
fn display_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Split one visual line's `seg_len` chars into styled zones (CMDHINT_01).
///
/// `token` is a char range in the LOGICAL line; `vl` gives the window this visual line slices out of
/// it. Returns at most three `(start_in_seg, len, style)` zones — `base` head, `accent` token, `base`
/// tail — covering `0..seg_len` contiguously and left-to-right; `None` slots are absent zones. Only
/// the visual line(s) overlapping the token produce a non-trivial split — every other line gets one
/// `base` zone, exactly what the code did before. This is the one genuinely new geometry case: a
/// long command name wrapped by `word_wrap_line` across two visual lines must stay highlighted on
/// both. `word_wrap_line` returns `(start, len)` pairs that tile the logical line exactly (`:2296`
/// pushes the final `(chunk_start, n - chunk_start)`; `visual_line_map:551-552` converts them), so
/// every char lands in exactly one window and the intersection below is total.
///
/// The HEAD zone is unreachable under today's invariant and kept deliberately: `token.start` is
/// always 0 (the `/` is char 0 of line 0), so `lo == win_start` and `a == 0` on every window. Keeping
/// the slot makes this a total function of an arbitrary contiguous range rather than one that
/// silently assumes a zero start — do not "simplify" it away, and do not be alarmed when manual
/// testing never exercises it.
///
/// A fixed ARRAY, not a `Vec`: called once per VISIBLE visual line on every frame (~20/frame), and
/// the ≤ 3 bound is structural, so the heap allocation buys nothing. See the perf section.
fn style_zones(
    vl: &VisualLine,
    seg_len: usize,
    token: Option<&std::ops::Range<usize>>,
    base: Style,
    accent: Style,
) -> [Option<(usize, usize, Style)>; 3] {
    let plain = [Some((0usize, seg_len, base)), None, None];
    // Only logical line 0 ever carries a command token.
    let Some(tok) = token.filter(|_| vl.logical == 0) else { return plain };
    let win_start = vl.start;
    let win_end = win_start.saturating_add(seg_len);
    let lo = tok.start.max(win_start);
    let hi = tok.end.min(win_end);
    if lo >= hi {
        return plain;
    }
    let (a, b) = (lo.saturating_sub(win_start), hi.saturating_sub(win_start));
    [
        (a > 0).then_some((0, a, base)),
        Some((a, b.saturating_sub(a), accent)),
        (b < seg_len).then_some((b, seg_len.saturating_sub(b), base)),
    ]
}

/// Build one visual line's spans from its style zones, overlaying the reverse-video soft cursor when
/// `cursor` is `Some(col_within_seg)` (CMDHINT_01 restructure of the old cursor-overlay body).
///
/// The cursor cell is one whole GRAPHEME, not one char — pi takes `afterGraphemes[0].segment`
/// (`editor.ts:555-559`), so a ZWJ emoji inverts as a unit. The cluster is therefore measured
/// against the WHOLE remaining segment, never the zone, exactly as the code being replaced does.
/// That cannot straddle a zone edge; see the straddle proof in the task notes — the short form is
/// that the only non-trivial zone edge sits on a whitespace char, and no grapheme cluster spans
/// whitespace (GB3's `CR LF` is the sole exception and the buffer cannot contain `\r`). The `after`
/// slice is still clamped to the zone, which costs one `saturating_sub` and makes the property
/// enforced rather than merely argued.
fn spans_for_segment(
    seg: &[char],
    zones: &[Option<(usize, usize, Style)>],
    cursor: Option<usize>,
    cursor_style: Style,
    base: Style,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    // `.flatten()` skips absent zones; the remaining ones are still left-to-right, which is what the
    // resulting span order depends on.
    for &(start, len, style) in zones.iter().flatten() {
        let end = start.saturating_add(len);
        match cursor.filter(|c| *c >= start && *c < end) {
            Some(c) => {
                let before: String =
                    seg.iter().skip(start).take(c.saturating_sub(start)).collect();
                if !before.is_empty() {
                    spans.push(Span::styled(before, style));
                }
                let tail: String = seg.iter().skip(c).collect();
                match tail.graphemes(true).next() {
                    Some(g) => {
                        let after_at = c.saturating_add(g.chars().count());
                        spans.push(Span::styled(g.to_string(), cursor_style));
                        let after: String = seg
                            .iter()
                            .skip(after_at)
                            .take(end.saturating_sub(after_at))
                            .collect();
                        if !after.is_empty() {
                            spans.push(Span::styled(after, style));
                        }
                    }
                    None => spans.push(Span::styled(" ", cursor_style)),
                }
            }
            None => {
                let text: String = seg.iter().skip(start).take(len).collect();
                if !text.is_empty() {
                    spans.push(Span::styled(text, style));
                }
            }
        }
    }
    // End-of-line caret: the cursor sits one past the last char (pi `editor.ts:563`). This is also
    // the whole-line case for an empty visual line, whose only zone is zero-length — the loop above
    // takes the `None` arm there (`0 >= 0 && 0 < 0` is false) and emits nothing, so this push is the
    // caret's only producer for an empty buffer.
    if cursor == Some(seg.len()) {
        spans.push(Span::styled(" ", cursor_style));
    }
    if spans.is_empty() {
        // An empty visual line that is NOT the cursor's. Today's code pushes an empty `base` span
        // here — keep `base`, not `cursor_style`, or a blank soft-newline row grows a stray caret.
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

/// The dim ghost span for `hint`, clipped to `available` columns (CMDHINT_01).
///
/// Structurally safe by two independent mechanisms, which is why this is an affordance rather than a
/// layout guard: (1) the render `Paragraph` has **no** `.wrap(…)`, so ratatui truncates a
/// too-long `Line` instead of reflowing it — the ghost can never add a row; (2) the editor's height
/// comes from the wrap map of REAL buffer content (`visual_line_count`, `:567-571` ←
/// `app/layout.rs:71-77`), which the ghost is not part of. Clipped to `available - 1` chars plus `…`;
/// a single column is `…`.
fn ghost_span(hint: &str, available: usize, style: Style) -> Option<Span<'static>> {
    if available == 0 {
        return None;
    }
    let n = hint.chars().count();
    let text = if n <= available {
        hint.to_string()
    } else if available == 1 {
        "…".to_string()
    } else {
        let mut s: String = hint.chars().take(available.saturating_sub(1)).collect();
        s.push('…');
        s
    };
    Some(Span::styled(text, style))
}

/// Whether a grapheme counts as whitespace — Pi's `isWhitespaceChar`, which is literally
/// `/\s/.test(char)` (`utils.ts:943-945`), i.e. *contains* a whitespace code point rather than *is*
/// one, hence `any` and not `all`.
///
/// [CYRUP-DELTA] JS `\s` and Rust's `char::is_whitespace` (Unicode `White_Space`) differ on exactly
/// two code points in either direction: `\s` includes U+FEFF (which `White_Space` does not) and
/// `White_Space` includes U+0085 NEL (which `\s` does not). Both are stripped before they can reach
/// the buffer — U+FEFF and U+0085 are `char::is_control()`/format characters that
/// [`sanitize_paste`] drops and that no key event produces — so the sets coincide on every input
/// this function can actually see.
fn is_whitespace_seg(g: &str) -> bool {
    g.chars().any(char::is_whitespace)
}

/// Whether a grapheme offers a CJK line-break opportunity — Pi's `cjkBreakRegex` (`utils.ts:54-55`):
///
/// ```text
/// /[\p{Script_Extensions=Han}\p{Script_Extensions=Hiragana}\p{Script_Extensions=Katakana}
///   \p{Script_Extensions=Hangul}\p{Script_Extensions=Bopomofo}]/u
/// ```
///
/// Tested against the whole grapheme (the regex is unanchored, so it matches if ANY code point in
/// the cluster qualifies) — hence `any`, matching `cjkBreakRegex.test(grapheme)`.
///
/// [CYRUP-DELTA] Rust's standard library carries no `Script_Extensions` data and pulling in a
/// unicode-script crate for one predicate would be a new external dependency for a table that does
/// not move (these five scripts' blocks are stable since Unicode 13). The ranges below are the
/// assigned blocks of those five scripts plus the shared-ideographic code points
/// (`〄〇〡-〩〸-〻`) that `Script_Extensions=Han` picks up beyond `Script=Han`.
fn is_cjk_break(g: &str) -> bool {
    g.chars().map(u32::from).any(|c| {
        matches!(c,
            0x1100..=0x11FF   // Hangul Jamo
            | 0x2E80..=0x2EFF // CJK Radicals Supplement       (Han)
            | 0x2F00..=0x2FDF // Kangxi Radicals               (Han)
            | 0x3005          // 々 ideographic iteration mark (Han)
            | 0x3007          // 〇 ideographic number zero    (Han)
            | 0x3021..=0x3029 // 〡-〩 Hangzhou numerals        (Han)
            | 0x3038..=0x303B // 〸-〻                          (Han)
            | 0x3041..=0x309F // Hiragana (incl. the shared voiced-sound marks)
            | 0x30A0..=0x30FF // Katakana
            | 0x3100..=0x312F // Bopomofo
            | 0x3130..=0x318F // Hangul Compatibility Jamo
            | 0x31A0..=0x31BF // Bopomofo Extended
            | 0x31F0..=0x31FF // Katakana Phonetic Extensions
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xA960..=0xA97F // Hangul Jamo Extended-A
            | 0xAC00..=0xD7AF // Hangul Syllables
            | 0xD7B0..=0xD7FF // Hangul Jamo Extended-B
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFE30..=0xFE4F // CJK Compatibility Forms       (Han)
            | 0xFF66..=0xFF9F // Halfwidth Katakana
            | 0xFFA0..=0xFFDC // Halfwidth Hangul
            | 0x1B000..=0x1B16F // Kana Supplement / Extended-A / Small Kana Extension
            | 0x20000..=0x2FA1F // CJK Extensions B-F + Compatibility Supplement
            | 0x30000..=0x323AF // CJK Extensions G-H
        )
    })
}

/// Word-aware wrap of one logical line into `(start_col, len)` visual segments fitting `width`
/// **display columns** — a 1:1 port of `wordWrapLine` (`editor.ts:114-206`). An empty line yields
/// one zero-length segment. `width` is assumed `>= 1` (callers clamp). The returned columns are
/// char indices into `line` (cyrup's buffer is `Vec<char>`, so char indices are its `string.slice`),
/// and the segments tile the line contiguously.
///
/// The three things the previous implementation got wrong, all from measuring `n - start <= width`
/// over a `&[char]` — a CHAR COUNT:
///
/// 1. **Width.** Upstream accumulates `visibleWidth(grapheme)` (`:139-143`), so 24 CJK ideographs
///    are 48 columns, not 24. At a layout width of 39 the char count said "fits", the map reported
///    one visual line, four ideographs rendered past the right edge and — because
///    [`Self::cursor_in`] resolves the caret through that same map — the caret left the frame.
/// 2. **Granularity.** Upstream iterates GRAPHEMES and breaks at a cluster's own start index
///    (`:157-160`), so a break never lands inside a cluster. Breaking at `start + width` char-wise
///    put `👨` on one row and a bare `\u{200d}👩‍👧‍👦` on the next.
/// 3. **CJK break opportunities.** Upstream records a wrap opportunity between any two adjacent
///    non-space graphemes when either is CJK (`:191-198`), because CJK text has no spaces to break
///    at. Without it a whole CJK paragraph is one unbreakable "word".
///
/// The loop below is upstream's, statement for statement: an overflow check that first tries to
/// backtrack to the last recorded opportunity and otherwise force-breaks at the current cluster's
/// start (`:145-161`), then the advance and the opportunity bookkeeping (`:180-199`).
///
/// [CYRUP-DELTA] `:163-178` handles a single segment wider than `maxWidth` by *recursively*
/// re-wrapping it, which upstream needs because its segmenter merges a whole `[paste #N …]` marker
/// into one atomic segment. cyrup's segments are plain extended grapheme clusters — never composite
/// — so there is nothing to re-wrap: an over-wide cluster (a wide emoji at `width == 1`) is
/// indivisible and takes a row of its own. That is where upstream's recursion converges for a
/// splittable segment, and it is also the case upstream cannot express at all: `wordWrapLine("👨",
/// 1)` recurses on itself forever.
fn word_wrap_line(line: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let n = line.len();
    // `if (!line || maxWidth <= 0) return [{ text: "", startIndex: 0, endIndex: 0 }]` (`:115-117`).
    if n == 0 {
        return vec![(0, 0)];
    }
    let s: String = line.iter().collect();
    // `if (lineWidth <= maxWidth) return [{ text: line, ... }]` (`:119-122`).
    if display_width(&s) <= width {
        return vec![(0, n)];
    }

    // `const segments = [...graphemeSegmenter.segment(line)]` (`:125`), carrying each cluster's
    // start index — `seg.index` upstream, a char column here.
    let mut segs: Vec<(usize, &str)> = Vec::with_capacity(n);
    let mut col = 0usize;
    for g in s.graphemes(true) {
        segs.push((col, g));
        col += g.chars().count();
    }

    let mut chunks: Vec<(usize, usize)> = Vec::new();
    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    // `wrapOppIndex` / `wrapOppWidth` (`:131-133`), as one `Option` so `-1` cannot leak.
    let mut wrap_opp: Option<(usize, usize)> = None;

    for i in 0..segs.len() {
        let Some(&(char_index, grapheme)) = segs.get(i) else { continue };
        let g_width = display_width(grapheme);
        let is_ws = is_whitespace_seg(grapheme);

        // "Overflow check before advancing" (`:145-161`).
        if current_width + g_width > width {
            match wrap_opp {
                // "Backtrack to last wrap opportunity (the remaining content plus the current
                // grapheme still fits within maxWidth)" (`:147-153`).
                Some((opp_index, opp_width))
                    if current_width.saturating_sub(opp_width) + g_width <= width =>
                {
                    chunks.push((chunk_start, opp_index.saturating_sub(chunk_start)));
                    chunk_start = opp_index;
                    current_width = current_width.saturating_sub(opp_width);
                }
                // "No viable wrap opportunity: force-break at current position" (`:154-160`).
                _ if chunk_start < char_index => {
                    chunks.push((chunk_start, char_index - chunk_start));
                    chunk_start = char_index;
                    current_width = 0;
                }
                _ => {}
            }
            wrap_opp = None;
        }

        // `if (gWidth > maxWidth)` (`:163`) — see the [CYRUP-DELTA] above.
        if g_width > width {
            if chunk_start < char_index {
                chunks.push((chunk_start, char_index - chunk_start));
            }
            chunk_start = char_index;
            current_width = g_width;
            wrap_opp = None;
            continue;
        }

        // "Advance" (`:181`).
        current_width += g_width;

        // "Record wrap opportunity" (`:183-199`): whitespace followed by non-whitespace (multiple
        // spaces join; the break point is after the last space), or a boundary where either side is
        // CJK.
        if let Some(&(next_index, next)) = segs.get(i + 1)
            // Upstream spells this as two arms — whitespace→non-whitespace (`:187-189`) and the CJK
            // boundary (`:190-198`) — that assign the same pair. Merged into one predicate because
            // clippy's `if_same_then_else` rejects the duplicated arm; `is_ws || cjk || cjk` under a
            // shared `!next_is_ws` is exactly the disjunction of the two upstream guards.
            && !is_whitespace_seg(next)
            && (is_ws || is_cjk_break(grapheme) || is_cjk_break(next))
        {
            wrap_opp = Some((next_index, current_width));
        }
    }

    // "Push final chunk" (`:202`).
    chunks.push((chunk_start, n.saturating_sub(chunk_start)));
    chunks
}

/// Sanitize a bracketed-paste payload (`editor.ts:1142-1179`): normalize `\r\n`/`\r` to `\n`, expand
/// tabs to four spaces, and drop control bytes other than `\n`.
fn sanitize_paste(raw: &str) -> String {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(unified.len());
    for c in unified.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}


/// One scroll-indicator rule, a 1:1 port of `createScrollBorder` (`editor.ts:259-268`):
///
/// ```text
/// const indicator = `─── ${direction} ${hiddenLineCount} more `;
/// const remaining = availableWidth - visibleWidth(indicator);
/// if (remaining >= 0) return indicator + "─".repeat(remaining);
/// const ellipsis = "...".slice(0, availableWidth);
/// return sliceByColumn(indicator, 0, availableWidth - visibleWidth(ellipsis), true) + ellipsis;
/// ```
///
/// `direction` is `'↑'` (rows scrolled off the top) or `'↓'` (rows still below).
///
/// **The trailing `true` is `strict`, not a pad flag.** `sliceByColumn(line, startCol, length,
/// strict = false)` (`utils.ts:1195-1197`) forwards to `sliceWithWidth`, whose `strict` drops a
/// grapheme that would straddle the end column (`:1224`, `const fits = !strict || currentCol + w <=
/// endCol`); it returns `{ text, width }` and pads NOTHING. The `pad` parameter that does exist
/// upstream belongs to `truncateToWidth`/`finalizeTruncatedResult`, a different function. So
/// `createScrollBorder`'s fallback is not padded upstream and is not padded here — the loop below is
/// that strict slice, and it is equivalent statement for statement: upstream skips a non-fitting
/// grapheme and then breaks on `currentCol >= endCol`, which for a strictly-increasing column count
/// is the same set of graphemes this `break` keeps.
///
/// The result is nevertheless always exactly `width` display columns, and that is a property of the
/// indicator's ALPHABET rather than of the slice: `─`, the space, `↑`/`↓` (East-Asian Ambiguous,
/// hence narrow) and the decimal digits are every one of them a single column, so `strict` never has
/// a wide grapheme to reject. [`tests::the_scroll_rule_is_exactly_as_wide_as_it_is_asked_for`] pins
/// it across the whole width range, because cyrup — unlike pi, which composes each row from scratch
/// — paints this string OVER the `Block`'s already-drawn rule, and a short string would leak the
/// `─`s underneath.
fn scroll_border(direction: char, hidden: usize, width: u16) -> String {
    let avail = usize::from(width);
    let indicator = format!("─── {direction} {hidden} more ");
    let indicator_w = display_width(&indicator);
    if avail >= indicator_w {
        let mut out = indicator;
        out.push_str(&"─".repeat(avail - indicator_w));
        return out;
    }
    // Too narrow for the whole indicator: keep as many leading columns as fit, then `...` (itself
    // truncated to the available width on a truly tiny terminal).
    let ellipsis: String = "...".chars().take(avail).collect();
    let budget = avail.saturating_sub(display_width(&ellipsis));
    let mut out = String::new();
    let mut used = 0usize;
    for g in indicator.graphemes(true) {
        let w = display_width(g);
        if used + w > budget {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push_str(&ellipsis);
    out
}

/// The grapheme-cluster boundaries of `line` expressed as **char-column** indices, including the
/// leading `0` and trailing line length (`unicode_segmentation` extended grapheme clusters — the
/// boundaries Pi's editor steps the cursor over). A pure-ASCII line yields every column.
fn grapheme_boundaries(line: &[char]) -> Vec<usize> {
    let s: String = line.iter().collect();
    let mut bounds = Vec::with_capacity(line.len() + 1);
    bounds.push(0usize);
    let mut col = 0usize;
    for g in s.graphemes(true) {
        col += g.chars().count();
        bounds.push(col);
    }
    bounds
}

impl Component for InputEditor {
    /// Render the editor with **top + bottom rules only** (no side bars, no title) — Pi
    /// `editor.ts:476,517,575` (spec/tui/03 §3.1). The rule color flips to bash-green while the buffer
    /// starts with `!` (spec/tui/03 §7.1); otherwise it uses the border role, accented when focused.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // Record the layout width so vertical (visual-line) motion wraps the same way it is drawn.
        // The editor has no side borders; one column is reserved for the end-of-line cursor cell
        // (`editor.ts:471` `layout_width = content_width - 1`) — unless `editorPaddingX` gave the
        // caret padding to overflow into (`editor.ts:489`).
        let pad = self.effective_padding(area.width);
        self.view_width = self.layout_width(area.width) as usize;
        // The rule color is the primary always-visible mode signal (spec/tui/03 §3.3): bash-green
        // while the buffer starts with `!`, else the escalating thinking-level color. The previous
        // hardwired bright-blue accent-on-focus was wrong (audit #3).
        let rule_style = if self.is_bash_mode() {
            theme.bash_mode_style()
        } else if self.thinking_level_owns_border {
            theme.thinking_border_style(&self.thinking_level)
        } else {
            // T9: an editor nobody reassigned keeps `getEditorTheme().borderColor` = `borderMuted`
            // (Pi `theme.ts:1301-1304` → `tui/src/components/editor.ts:348`).
            theme.border_muted_style()
        };
        // `editorPaddingX` insets the TEXT only: ratatui's `Block` draws its top/bottom rules across
        // the full `area` and applies `Padding` to the inner area the `Paragraph` fills, which is
        // exactly Pi's split (`editor.ts:522` pads the text rows; `:530` repeats the rule glyph
        // `width` times).
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_set(border::PLAIN)
            .border_style(rule_style)
            .padding(Padding::horizontal(pad));
        // A reverse-video soft cursor cell makes the caret visible every idle frame (Pi
        // `editor.ts:545-564`). Without it the body row paints blank, because the hardware cursor
        // (`set_cursor_position`) is invisible in a headless buffer.
        //
        // **E1 — there is no prompt glyph.** cyrup used to open row 0 with an accent `› `. Pi's
        // `Editor.render` (`editor.ts:482-601`) emits only `${leftPadding}${displayText}${padding}
        // ${lineRightPadding}` (`:578`); nothing anywhere in the chat editor's construction adds a
        // leading glyph — the chat editor is a bare `new CustomEditor(this.ui, getEditorTheme(),
        // this.keybindings, {…})` (`interactive-mode.ts:563-566`) and `CustomEditor`
        // (`components/custom-editor.ts`, 90 lines) overrides `handleInput` ONLY, with no `render`.
        // The `›` upstream *does* draw is the SELECTED-ROW cursor of the list selectors
        // (`session-selector.ts:476`, `tree-selector.ts:689`, `user-message-selector.ts:57`), a
        // different component. Removing it also fixes **E2**: row 0 was `PROMPT_W + view_width`
        // columns wide inside a `view_width`-wide area (last character clipped, end-of-line caret off
        // the right edge) while rows 1..n started two columns to its left — a permanent ragged left
        // edge. Every row now starts flush at `leftPadding`, exactly as `:578` does.
        let base = theme.base_style();
        let cursor_style = base.add_modifier(Modifier::REVERSED);
        // Expand each LOGICAL line into its wrapped VISUAL lines at `view_width` (`editor.ts:1690`
        // `build_visual_line_map`, the same primitive vertical motion uses) and emit one ratatui
        // `Line` per visual line — so text past the width flows onto the next row instead of clipping
        // (the `Paragraph` has no `.wrap`, so it renders exactly the rows we build). The soft cursor
        // rides its VISUAL row/col, not the logical column.
        let map = self.visual_line_map();
        // **E13 — the caret survives focus loss.** Pi gates `focused` on the zero-width hardware
        // `CURSOR_MARKER` alone (`editor.ts:537,550`); the reverse-video cell is emitted purely from
        // `layoutLine.hasCursor`, which `layoutText` sets from the cursor position and never consults
        // `focused` (`editor.ts:905-960`). cyrup used to set `cursor_vl = usize::MAX` when unfocused,
        // so clicking away from the terminal (`FocusLost`) erased the caret entirely. The
        // focus-gated half lives on in [`Self::cursor_in`], which is cyrup's `CURSOR_MARKER`.
        let cursor_vl = self.current_visual_line(&map);
        // **E4 — the visible window scrolls; it does not clip.** Pi slices `layoutLines` to
        // `maxVisibleLines` after moving `scrollOffset` to keep the caret inside
        // (`editor.ts:499-519`).
        //
        // **E17 — the cap is the component's own.** Upstream reads `this.tui.terminal.rows` inside a
        // `render(width)` that takes no height (`:499-501`); the budget is intrinsic and the
        // container is never consulted. cyrup took `area.height - 2` alone — correct only for as
        // long as the one caller happened to size the slot from the same formula, and silently
        // uncapped for every other caller. It is now `min(rect, intrinsic)`: the intrinsic budget is
        // pi's, and the rect stays in the `min` so a slot CLIPPED shorter than the editor asked for
        // still degrades correctly rather than overdrawing its neighbours.
        let max_visible = usize::from(area.height.saturating_sub(2))
            .min(usize::from(self.max_visible_lines()))
            .max(1);
        if cursor_vl < self.scroll_offset {
            self.scroll_offset = cursor_vl;
        } else if cursor_vl >= self.scroll_offset.saturating_add(max_visible) {
            self.scroll_offset = cursor_vl.saturating_add(1).saturating_sub(max_visible);
        }
        self.scroll_offset = self.scroll_offset.min(map.len().saturating_sub(max_visible));
        let mut lines: Vec<Line> = Vec::with_capacity(max_visible);
        let highlight = self.command_highlight();          // computed ONCE per frame
        let accent = theme.accent_style();
        let dim = theme.dim_style();
        let last_vi = map.len().saturating_sub(1);
        // The Block's inner width — the true drawable span for the ghost. NOT `self.view_width`:
        // `layout_width` subtracts one column for the caret when `paddingX == 0`, so `view_width`
        // under-counts the inner area by one in the default configuration. The formula is exactly
        // `area.width - 2 * pad` because the Block carries `Borders::TOP | BOTTOM` only — no side
        // border steals a column — and `Padding::horizontal(pad)` with `pad = effective_padding(...)`.
        let inner_w = usize::from(area.width.saturating_sub(pad.saturating_mul(2))).max(1);
        for (vi, vl) in map.iter().enumerate().skip(self.scroll_offset).take(max_visible) {
            // The chars this visual line slices out of its logical line.
            let seg: Vec<char> = self
                .lines
                .get(vl.logical)
                .map(|l| l.iter().skip(vl.start).take(vl.len).copied().collect())
                .unwrap_or_default();
            let zones = style_zones(vl, seg.len(), highlight.as_ref().map(|h| &h.token), base, accent);
            let cursor = (vi == cursor_vl).then(|| self.col.saturating_sub(vl.start).min(seg.len()));
            let mut spans = spans_for_segment(&seg, &zones, cursor, cursor_style, base);
            // The ghost trails the buffer's LAST visual line, after the real content and AFTER the
            // caret cell. It is not buffer content, so it never joins the cursor split and the cursor
            // can never sit inside it. Deliberately after the caret, not under it: cyrup's caret is a
            // reverse-video BLOCK, and a dim hint char inverted beneath it would read as
            // already-typed text — the opposite of what a placeholder must say.
            if vi == last_vi
                && let Some(hint) = highlight.as_ref().and_then(|h| h.ghost.as_deref())
            {
                // Charge COLUMNS, not chars — and take them from the spans just built, so the caret
                // cell (a `" "` span, or the inverted grapheme) is counted by construction rather
                // than re-derived, and no intermediate `String` is allocated.
                let used: usize = spans.iter().map(Span::width).sum();
                if let Some(span) = ghost_span(hint, inner_w.saturating_sub(used), dim) {
                    spans.push(span);
                }
            }
            lines.push(Line::from(spans));
        }
        let shown = lines.len();
        let para = Paragraph::new(lines).block(block).style(base);
        frame.render_widget(para, area);
        // E4's other half: the rules ANNOUNCE the hidden rows. `createScrollBorder`
        // (`editor.ts:259-268`) replaces the plain `─`-repeat with `─── ↑ N more ───…` at the top
        // when `scrollOffset > 0` (`:526-528`) and `─── ↓ N more ───…` at the bottom when content
        // remains below (`:582-585`). The `Block` above already painted a plain rule on both edges;
        // these overwrite it in place, which is byte-identical to pi choosing one string or the other
        // (both are exactly `width` columns).
        if self.scroll_offset > 0 && area.height >= 1 {
            let text = scroll_border('↑', self.scroll_offset, area.width);
            let row = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(text, rule_style))), row);
        }
        let below = map.len().saturating_sub(self.scroll_offset.saturating_add(shown));
        if below > 0 && area.height >= 2 {
            let text = scroll_border('↓', below, area.width);
            let row = Rect {
                x: area.x,
                y: area.y.saturating_add(area.height.saturating_sub(1)),
                width: area.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(text, rule_style))), row);
        }
        // Pi hides the terminal's real cursor unless `showHardwareCursor` is on (`tui.ts:1659-1663`
        // `if (this.showHardwareCursor) showCursor() else hideCursor()`); ratatui's `Terminal::draw`
        // hides it for us whenever no position was set, so the gate is the call itself.
        if self.show_hardware_cursor
            && let Some((x, y)) = self.cursor_in(area)
        {
            frame.set_cursor_position((x, y));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    /// `word_wrap_line` over a `&str`, as `(chunk text)` — the shape `wordWrapLine` returns
    /// (`editor.ts:114-206` yields `{ text, startIndex, endIndex }`).
    fn wrap(s: &str, width: usize) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        word_wrap_line(&chars, width)
            .into_iter()
            .map(|(start, len)| chars.iter().skip(start).take(len).collect())
            .collect()
    }

    /// The chunks tile the line: contiguous, gap-free, and reassembling to the input. Upstream gets
    /// this for free from `line.slice(chunkStart, …)` + a final `line.slice(chunkStart)` (`:202`).
    fn assert_tiles(s: &str, width: usize) {
        let chars: Vec<char> = s.chars().collect();
        let segs = word_wrap_line(&chars, width);
        let mut at = 0usize;
        for (start, len) in &segs {
            assert_eq!(*start, at, "chunk {segs:?} of {s:?}@{width} is not contiguous");
            at += len;
        }
        assert_eq!(at, chars.len(), "chunks {segs:?} do not cover {s:?}");
    }

    // -------------------------------------------------------------- wordWrapLine ---------------

    /// The two early returns (`editor.ts:115-122`): an empty line is one empty chunk, and a line
    /// that already fits is one chunk covering the whole line.
    #[test]
    fn a_line_that_fits_is_one_chunk() {
        assert_eq!(word_wrap_line(&[], 10), vec![(0, 0)]);
        assert_eq!(wrap("hello", 10), vec!["hello"]);
        // "fits" is measured in COLUMNS: 5 ideographs are 10 of them.
        assert_eq!(wrap("日本語です", 10), vec!["日本語です"]);
        assert_eq!(wrap("日本語です", 9).len(), 2, "…and 9 columns is one short");
    }

    /// Whitespace is the primary wrap opportunity, and the break lands AFTER the space run so the
    /// trailing space stays on the wrapped row (`wrapOppIndex = next.index`, `editor.ts:187-189`).
    #[test]
    fn wrapping_breaks_after_the_last_space_that_fits() {
        assert_eq!(wrap("aaa bbb ccc", 5), vec!["aaa ", "bbb ", "ccc"]);
        assert_eq!(wrap("aaa  bbb", 5), vec!["aaa  ", "bbb"], "a run of spaces joins (`:187`)");
        // And it is GREEDY, not balanced: at width 7 the tail `"bbb ccc"` is exactly 7 columns, so
        // the backtrack at the second space finds it already fits and never fires
        // (`currentWidth - wrapOppWidth + gWidth <= maxWidth`, `editor.ts:147`).
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa ", "bbb ccc"]);
        assert_tiles("aaa bbb ccc", 5);
        assert_tiles("aaa bbb ccc", 7);
    }

    /// A word longer than the width force-breaks at the current grapheme's own start index
    /// (`editor.ts:154-160`).
    #[test]
    fn an_overlong_word_force_breaks_at_the_width() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_tiles("abcdefghij", 4);
    }

    /// **The display-width bug.** `visibleWidth(grapheme)` (`editor.ts:139-143`), not a char count:
    /// 24 ideographs are 48 columns and cannot be one 39-column row. The break also needs
    /// `cjkBreakRegex` (`utils.ts:54`, used at `editor.ts:191-198`) — CJK has no spaces to break at,
    /// so without the CJK opportunity the whole run would be one unbreakable "word".
    #[test]
    fn cjk_is_measured_and_broken_in_columns() {
        let cjk: String = "日本語".chars().cycle().take(24).collect();
        let rows = wrap(&cjk, 39);
        assert_eq!(rows.len(), 2, "48 columns do not fit 39: {rows:?}");
        assert_eq!(rows[0].chars().count(), 19, "19 ideographs are 38 columns; a 20th would be 40");
        assert_eq!(rows.concat(), cjk);
        assert_tiles(&cjk, 39);
        for r in &rows {
            assert!(display_width(r) <= 39, "row overflows: {r:?}");
        }
    }

    /// The CJK opportunity is a BOUNDARY rule — it fires when either side is CJK
    /// (`editor.ts:194-198`), so Latin text abutting CJK may break between them.
    #[test]
    fn a_latin_cjk_boundary_is_a_wrap_opportunity() {
        // `word` is 4 columns and each ideograph 2, so at width 4 the opportunity recorded at the
        // `d`→`日` boundary is what puts `word` on a row of its own; the two that follow come from
        // the CJK-to-CJK opportunities.
        let rows = wrap("word日本語", 4);
        assert_eq!(rows, vec!["word", "日本", "語"], "the boundary breaks: {rows:?}");
        assert_tiles("word日本語", 4);
        // Contrast: an all-Latin run of the same length has NO opportunity anywhere, so it
        // force-breaks mid-"word" instead.
        assert_eq!(wrap("wordabcdef", 4), vec!["word", "abcd", "ef"]);
    }

    /// A grapheme CLUSTER is atomic: the break never lands inside one, whatever the width.
    #[test]
    fn a_cluster_is_never_split() {
        const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let line = format!("{}{FAMILY}", "a".repeat(38));
        let rows = wrap(&line, 39);
        assert_eq!(rows, vec!["a".repeat(38), FAMILY.to_string()], "torn cluster: {rows:?}");
        assert_tiles(&line, 39);

        // …including when the cluster ALONE is wider than the width: it is indivisible, so it takes
        // a row of its own rather than recursing forever the way `editor.ts:163-178` would.
        let rows = wrap(&format!("ab{FAMILY}cd"), 1);
        assert_eq!(rows, vec!["a", "b", FAMILY, "c", "d"], "{rows:?}");
        assert_tiles(&format!("ab{FAMILY}cd"), 1);
    }

    /// Every produced row fits, and the rows always tile the input — swept over a spread of widths
    /// and mixed scripts, because the failure mode of the old char-count wrap was silent overflow
    /// rather than a crash.
    #[test]
    fn every_wrapped_row_fits_its_width() {
        const CASES: [&str; 5] = [
            "the quick brown fox jumps over the lazy dog",
            "日本語のテキストは空白で区切られていません",
            "mixed 日本語 and latin with  double  spaces",
            "e\u{301}combining\u{301}marks\u{301}everywhere",
            "supercalifragilisticexpialidocious",
        ];
        for s in CASES {
            for width in 1..=45usize {
                assert_tiles(s, width);
                for row in wrap(s, width) {
                    // A single over-wide cluster is the one legal exception (see above): it cannot
                    // be split, so it is emitted alone.
                    let clusters = row.graphemes(true).count();
                    assert!(
                        display_width(&row) <= width || clusters == 1,
                        "{s:?}@{width}: row {row:?} is {} columns",
                        display_width(&row)
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------- createScrollBorder ------------

    /// The wide path (`editor.ts:262-263`): the indicator, then `─` to the requested width.
    #[test]
    fn the_scroll_rule_reads_as_an_indicator_padded_with_rule() {
        let s = scroll_border('↑', 6, 20);
        assert!(s.starts_with("─── ↑ 6 more "), "{s:?}");
        assert_eq!(display_width(&s), 20, "{s:?}");
        assert!(s.ends_with('─'), "the remainder is rule, not blank: {s:?}");
    }

    /// The narrow path (`editor.ts:265-267`): a strict slice of the indicator plus `...`, itself
    /// clipped on a terminal too narrow even for that.
    #[test]
    fn a_terminal_too_narrow_for_the_indicator_gets_an_ellipsis() {
        assert_eq!(scroll_border('↓', 5, 10), "─── ↓ 5...", "{:?}", scroll_border('↓', 5, 10));
        assert_eq!(scroll_border('↓', 5, 2), "..");
        assert_eq!(scroll_border('↓', 5, 0), "");
    }

    /// The invariant the render depends on: the string is EXACTLY `width` columns for every width
    /// and every hidden count, so it overwrites the `Block`'s pre-painted rule with no `─` leaking
    /// out from underneath. (Upstream can be one column short here — `strict` may reject a wide
    /// grapheme at the boundary and nothing pads afterwards — but the indicator's alphabet is
    /// entirely single-column, so the case does not arise. See [`scroll_border`].)
    #[test]
    fn the_scroll_rule_is_exactly_as_wide_as_it_is_asked_for() {
        for direction in ['↑', '↓'] {
            for hidden in [0usize, 1, 9, 10, 99, 1234, 1_000_000] {
                for width in 0..=120u16 {
                    let s = scroll_border(direction, hidden, width);
                    assert_eq!(
                        display_width(&s),
                        usize::from(width),
                        "scroll_border({direction:?}, {hidden}, {width}) = {s:?}"
                    );
                }
            }
        }
    }
}
