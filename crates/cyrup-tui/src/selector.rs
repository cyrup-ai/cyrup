//! The editor-swap selector engine (spec/tui/05 §1.1, §3; port of Pi's `showSelector`
//! `interactive-mode.ts:3922-3933` + the `*-selector.ts` components).
//!
//! Pi's first-party selectors are **not** floating overlays: they *replace the input editor in place*
//! in the bottom inline region, full-width, delimited top and bottom by a `DynamicBorder`
//! (`dynamic-border.ts` — a full-width `─` rule, no box corners), and they push the message history up
//! (spec/tui/05 §1.1, §11). This module realizes that as the [`Selector`] trait (the input-slot
//! occupant) plus a shared [`ListSelector`] engine over [`SelectList`](crate::select_list::SelectList),
//! and the three dependency-free selectors Pi opens this way: thinking (`thinking-selector.ts`),
//! show-images (`show-images-selector.ts`), and theme with live preview (`theme-selector.ts`).
//!
//! The floating `OverlayManager` z-stack (spec/tui/05 §2) backs only extension-custom UI + the
//! hotkeys/help popup and is gated to the outer (L7) layer — the 13 first-party selectors are all
//! editor-swap, exactly as Pi (§1.2 "Decision for parity").

use cyrup_resources::theme::builtin_themes;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{ModelsAction, ModelsKeymap, SelectAction, SelectKeymap};
use crate::select_list::{ColumnLayout, SelectItem, SelectList};
use crate::theme::UiTheme;

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
pub fn input_line_spans(value: &str, cursor: usize, theme: &UiTheme) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(4);
    spans.push(Span::styled(INPUT_PROMPT, theme.base_style()));
    spans.extend(search_input_spans(value, cursor, theme));
    spans
}

/// Render an embedded selector **search `Input`** with a visible block cursor at the byte offset
/// `cursor` (feature #9 "selector IME cursor"). Pi's selector search boxes render a reverse-video
/// cursor (an `Input` component) so the caret + any IME pre-edit is visible; cyrup's selectors tracked
/// the cursor offset but never drew it, leaving the search box caret-less. The character under the
/// caret (or a trailing space when the cursor is at the end) is drawn reversed over the base style;
/// text before/after keeps the base style. Shared by the model / session / scoped search boxes.
pub fn search_input_spans(query: &str, cursor: usize, theme: &UiTheme) -> Vec<Span<'static>> {
    let cursor = cursor.min(query.len());
    // Snap to a char boundary so slicing never panics on a multi-byte caret position.
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

/// Which first-party selector occupies the input slot (spec/tui/05 §7 `SelectorKind`). The chrome
/// interprets a [`SelectorOutcome::Confirm`] / [`SelectorOutcome::Preview`] against this kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectorKind {
    /// Reasoning-level picker (`thinking-selector.ts`).
    Thinking,
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
    /// Cursor position for an embedded search `Input` (none for these pure-list selectors).
    fn cursor(&self) -> Option<(u16, u16)> {
        None
    }
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
}

/// The shared list-selector engine (spec/tui/05 §3.2 `ListView<T>`): a [`SelectList`] body wrapped in
/// the top/bottom `DynamicBorder` chrome, with a parallel `values` vector returned on confirm and an
/// optional live-preview hook.
pub struct ListSelector {
    list: SelectList,
    /// Confirm value per row, parallel to the list items (`SelectItem.value`, e.g.
    /// `thinking-selector.ts:35` `value: level`).
    values: Vec<String>,
    /// Whether a selection move emits [`SelectorOutcome::Preview`] (theme live preview only).
    preview: bool,
    /// An optional bold title rendered between the top rule and the list (`*-selector.ts` headers).
    title: Option<String>,
    /// The live selector bindings, so the hint row names the keys the user actually has bound
    /// (`keyHint` → `keyText` → `getKeybindings().getKeys(...)`, `keybinding-hints.ts:34-44`).
    /// Defaults to the stock table; [`ListSelector::with_hints`] adopts the app's merged one, and
    /// [`Selector::handle`] refreshes it from whatever keymap actually routed the key.
    keymap: SelectKeymap,
    /// Whether to draw the keyboard-hint row — OPT-IN, see [`SelectorKind::draws_hint_row`].
    hints: bool,
    /// Whether to inset the body one column — OPT-IN, see [`SelectorKind::insets_rows`].
    inset: bool,
    /// Whether to draw the envelope's `Spacer(1)` rows — OPT-IN, see
    /// [`SelectorKind::envelope_spacers`].
    spacers: bool,
}

impl ListSelector {
    /// Build from `(value, label, description)` rows, the max visible window, and whether the selector
    /// previews on navigation. The selection preselects `selected`. Column layout is Pi's selector
    /// default `{min:12,max:32}` (`THINKING_SELECT_LIST_LAYOUT` etc.).
    fn new(
        rows: Vec<(String, String, Option<String>)>,
        max_visible: u16,
        selected: usize,
        preview: bool,
    ) -> Self {
        let mut values = Vec::with_capacity(rows.len());
        let mut items = Vec::with_capacity(rows.len());
        for (value, label, desc) in rows {
            values.push(value);
            items.push(SelectItem::new(label, desc));
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH);
        list.set_max_visible(max_visible);
        list.set_selected(selected);
        ListSelector {
            list,
            values,
            preview,
            title: None,
            keymap: SelectKeymap::default(),
            hints: false,
            inset: false,
            spacers: false,
        }
    }

    /// A data-bound selector (`model`/`session`/`tree`/… — `*-selector.ts`): build the windowed list
    /// from `(value, label, description)` rows sourced from an L5 service (model catalog, session list,
    /// branch tree), with a bold `title` header and a `no_match` empty-state line. Confirming yields the
    /// row's `value` for the run loop to apply (set model, switch branch, login…). `maxVisible = 10`
    /// matches the data selectors (`model-selector.ts:244`, `session-selector.ts`).
    pub fn data(
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
    ) -> Self {
        let empty = format!("No {} available", kind.title().to_lowercase());
        let mut values = Vec::with_capacity(rows.len());
        let mut items = Vec::with_capacity(rows.len());
        for (value, label, desc) in rows {
            values.push(value);
            items.push(SelectItem::new(label, desc));
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH).with_no_match(empty);
        list.set_max_visible(10);
        list.set_selected(selected);
        ListSelector {
            list,
            values,
            preview: false,
            title: Some(kind.title().to_string()),
            keymap: SelectKeymap::default(),
            hints: false,
            inset: false,
            spacers: false,
        }
    }

    /// A generic titled prompt (Pi `showStartupSelector`, startup-ui.ts:134-163): the pre-launch
    /// Continue/Cancel-style selector the bin mounts before the agent runtime is built (e.g. the
    /// missing-session-cwd prompt). Rows are `(value, label, description)`; confirming yields the
    /// highlighted row's value. `selected` preselects a row; `maxVisible` is the row count.
    pub fn prompt(title: String, rows: Vec<(String, String, Option<String>)>, selected: usize) -> Self {
        let count = rows.len().clamp(1, u16::MAX as usize) as u16;
        let mut selector = ListSelector::new(rows, count, selected, false);
        selector.title = Some(title);
        selector
    }

    /// **Opt in** to the keyboard-hint row, binding it to the app's live `tui.select.*` table so it
    /// names the keys the user has actually bound rather than the stock defaults (`keyHint`
    /// resolves through `keyText` on every render upstream, `keybinding-hints.ts:34-44`).
    ///
    /// Only the kinds whose pi component builds such a row may call this — see
    /// [`SelectorKind::draws_hint_row`] for the enumeration and the source lines behind it.
    /// [`Self::with_upstream_chrome`] applies it per-kind and is what callers normally want.
    pub fn with_hints(mut self, keymap: &SelectKeymap) -> Self {
        self.keymap = keymap.clone();
        self.hints = true;
        self
    }

    /// **Opt in** to the one-column row inset — see [`SelectorKind::insets_rows`].
    pub fn with_inset(mut self) -> Self {
        self.inset = true;
        self
    }

    /// **Opt in** to the envelope's `Spacer(1)` rows — see [`SelectorKind::envelope_spacers`].
    pub fn with_spacers(mut self) -> Self {
        self.spacers = true;
        self
    }

    /// The number of `Spacer(1)` rows this selector's envelope adds when it is drawing them at all:
    /// **four** with a hint row (`extension-selector.ts:45,49,62,74`), **three** without (the
    /// `oauth-selector.ts:69,74,93` subset that does not sit under a search `Input`; its fourth,
    /// `:87`, belongs to the `Input` cyrup has not ported).
    fn spacer_rows(&self) -> u16 {
        if !self.spacers {
            0
        } else if self.hints {
            4
        } else {
            3
        }
    }

    /// Apply exactly the chrome the pi component behind `kind` draws: the hint row iff
    /// [`SelectorKind::draws_hint_row`], the one-column inset iff [`SelectorKind::insets_rows`].
    ///
    /// This is the single place the per-kind decision is made. It exists because the previous batch
    /// made both a property of the shared [`ListSelector`] engine, which gave every dialog chrome
    /// that upstream draws on four of them (hint row) and six (inset) — `ThinkingSelectorComponent`
    /// is 75 lines of `DynamicBorder` + `SelectList` + `DynamicBorder` and has neither.
    pub fn with_upstream_chrome(mut self, kind: SelectorKind, keymap: &SelectKeymap) -> Self {
        if kind.draws_hint_row() {
            self = self.with_hints(keymap);
        }
        if kind.insets_rows() {
            self = self.with_inset();
        }
        if kind.envelope_spacers() {
            self = self.with_spacers();
        }
        self
    }

    /// The keyboard-hint row Pi's `ExtensionSelectorComponent` puts above the bottom border
    /// (`extension-selector.ts:63-73`): `rawKeyHint("↑↓","navigate") + "  " +
    /// keyHint("tui.select.confirm","select") + "  " + keyHint("tui.select.cancel","cancel")`,
    /// rendered as `new Text(..., 1, 0)` so it is inset one column.
    ///
    /// Each pair is two-tone — `dim` key, `muted` description (`keybinding-hints.ts:42-44`) — via
    /// [`crate::chrome::key_hint_spans`]. Keys come from [`SelectKeymap::keys_label`], which joins
    /// **all** bound keys with `/` exactly as upstream's `keyText` does, so the stock cancel hint
    /// reads `escape/ctrl+c cancel`, not just the first key.
    ///
    /// The `Spacer(1)` rows upstream places either side of this row are L4/SYS-3 and land with the
    /// rest of the dialog-envelope work; this adds the hint row itself.
    fn hint_line(&self, theme: &UiTheme) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(crate::chrome::key_hint_spans("↑↓", "navigate", theme));
        if let Some(keys) = self.keymap.keys_label(SelectAction::Confirm) {
            spans.push(Span::raw("  "));
            spans.extend(crate::chrome::key_hint_spans(&keys, "select", theme));
        }
        if let Some(keys) = self.keymap.keys_label(SelectAction::Cancel) {
            spans.push(Span::raw("  "));
            spans.extend(crate::chrome::key_hint_spans(&keys, "cancel", theme));
        }
        Line::from(spans)
    }

    /// The value of the currently-highlighted row (empty string if the list is empty — never panics).
    fn current_value(&self) -> String {
        self.values.get(self.list.selected()).cloned().unwrap_or_default()
    }

    /// Read-only access to the inner list (tests / chrome inspection).
    pub fn list(&self) -> &SelectList {
        &self.list
    }

    // ---- Pi selector constructors -----------------------------------------------------------

    /// Thinking-level picker (`thinking-selector.ts:11-55`): one row per available level with its
    /// token-estimate description, `maxVisible = levels.len()`, preselecting `current`.
    pub fn thinking(current: &str) -> Self {
        // `LEVEL_DESCRIPTIONS` (`thinking-selector.ts:11-19`), in Pi's order. Pi's `max` commit
        // (fbdd4638) renamed the `xhigh` copy from "Maximum" to "Extra-high" and gave "Maximum
        // reasoning" to the new top rung.
        const LEVELS: [(&str, &str); 7] = [
            ("off", "No reasoning"),
            ("minimal", "Very brief reasoning (~1k tokens)"),
            ("low", "Light reasoning (~2k tokens)"),
            ("medium", "Moderate reasoning (~8k tokens)"),
            ("high", "Deep reasoning (~16k tokens)"),
            ("xhigh", "Extra-high reasoning (~32k tokens)"),
            ("max", "Maximum reasoning"),
        ];
        let rows: Vec<_> = LEVELS
            .iter()
            .map(|(level, desc)| ((*level).to_string(), (*level).to_string(), Some((*desc).to_string())))
            .collect();
        let selected = LEVELS.iter().position(|(l, _)| *l == current).unwrap_or(0);
        ListSelector::new(rows, LEVELS.len().min(u16::MAX as usize) as u16, selected, false)
    }

    /// Inline-images yes/no (`show-images-selector.ts:19-31`): `maxVisible = 5`, preselecting
    /// `Yes` when currently on, else `No`.
    pub fn show_images(current: bool) -> Self {
        let rows = vec![
            ("yes".to_string(), "Yes".to_string(), Some("Show images inline in terminal".to_string())),
            ("no".to_string(), "No".to_string(), Some("Show text placeholder instead".to_string())),
        ];
        let selected = if current { 0 } else { 1 };
        ListSelector::new(rows, 5, selected, false)
    }

    /// Theme picker with live preview (`theme-selector.ts:27-56`): one row per available theme,
    /// `maxVisible = 10`, the current theme marked `(current)`, preselecting it. Navigation emits
    /// [`SelectorOutcome::Preview`] so the whole UI re-themes as the highlight moves.
    pub fn theme(current: &str) -> Self {
        let mut rows = Vec::new();
        let mut selected = 0usize;
        for (i, theme) in builtin_themes().iter().enumerate() {
            let key = theme.key.as_str().to_string();
            let is_current = key == current;
            if is_current {
                selected = i;
            }
            let desc = is_current.then(|| "(current)".to_string());
            rows.push((key.clone(), key, desc));
        }
        ListSelector::new(rows, 10, selected, true)
    }
}

impl Selector for ListSelector {
    fn desired_height(&self, width: u16) -> u16 {
        // Top `DynamicBorder` + optional (now auto-sizing, wrapped) title + list body + the hint row
        // **when this kind draws one** + bottom `DynamicBorder` (spec/tui/05 §3;
        // `extension-selector.ts:44-75`).
        let title_h = self.title.as_deref().map_or(0, |t| title_wrapped_height(t, width));
        let hint_h = u16::from(self.hints);
        self.list
            .rendered_height()
            .saturating_add(2)
            .saturating_add(hint_h)
            .saturating_add(title_h)
            .saturating_add(self.spacer_rows())
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = self.title.as_deref().map_or(0, |t| title_wrapped_height(t, area.width));
        let hint_h = u16::from(self.hints);
        // L4/SYS-3. The envelope row order is `ExtensionSelectorComponent`'s, counted from its
        // constructor (`extension-selector.ts:44-75`): `DynamicBorder`(:44) · `Spacer`(:45) ·
        // title(:47) · `Spacer`(:49) · list(:61) · `Spacer`(:62) · hint(:63-73) · `Spacer`(:74) ·
        // `DynamicBorder`(:75). `OAuthSelectorComponent` (`oauth-selector.ts:68-96`) is the same
        // order minus the hint row (it has none) — its `:87` spacer sits under a search `Input`
        // cyrup has not ported, so `sp_after_hint` collapses to 0 there and the count is three.
        // `spacers` is per-kind (`SelectorKind::envelope_spacers`); thinking/show-images/theme are
        // border/list/border upstream and keep a zero-spacer envelope.
        //
        // Every height below is the NATURAL one — `sp` does not depend on `area.height`, and the
        // body gets the list's own rendered height rather than "whatever is left". `stack_rows`
        // then fills the regions from the TOP and starves the trailing ones, which is what pi's
        // layout engine does; see its doc. The previous
        // `area.height - fixed` body made `fixed` count the hint unconditionally, so a three-row
        // slot spent its last row on the HINT and showed no options at all — the list starved
        // before the trailing chrome did, the exact inversion of upstream's order.
        let sp = u16::from(self.spacers);
        let sp_after_hint = sp.min(hint_h);
        let body_h = self.list.rendered_height();
        let [top, _, title_area, _, body, _, hint, _, bottom] = stack_rows(
            area,
            [1, sp, title_h, sp, body_h, sp, hint_h, sp_after_hint, 1],
        );
        frame.render_widget(border_rule(top.width, theme), top);
        if let Some(title) = &self.title {
            let style = theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD);
            frame.render_widget(
                Paragraph::new(title_lines(title))
                    .style(style)
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                title_area,
            );
        }
        // S28: where the pi component wraps its rows in `new Text(text, 1, 0)`
        // (`extension-selector.ts:87`) / `new TruncatedText(line, 1, 0)` (`oauth-selector.ts:144`),
        // the row gets a one-column left margin and a matching right one, and the list is laid out
        // in `contentWidth = max(1, width - paddingX * 2)` (`text.ts:64,70-76`) — hence `-2` here
        // and a single leading space. That reduced width is also what the two-column gate
        // (`select-list.ts:149` `width > 40`) then sees, which is correct for these kinds and
        // WRONG for the others: thinking / show-images / theme add the `SelectList` straight to the
        // container (`thinking-selector.ts:66`), so it is laid out at the full container width and
        // its rows start at column 0. Applying the inset unconditionally moved that gate by two
        // columns on every dialog.
        let lines = if self.inset {
            self.list
                .lines(body.width.saturating_sub(2), theme)
                .into_iter()
                .map(|line| {
                    let mut spans = vec![Span::raw(" ")];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect()
        } else {
            self.list.lines(body.width, theme)
        };
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), body);
        if self.hints {
            frame.render_widget(
                Paragraph::new(vec![self.hint_line(theme)]).style(theme.base_style()),
                hint,
            );
        }
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Keep the hint row honest even for a selector constructed without `with_keymap`: adopt
        // whatever table actually routed this key.
        self.keymap = keymap.clone();
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.list.select_up();
                self.moved()
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.list.select_down();
                self.moved()
            }
            Some(SelectAction::Confirm) => SelectorOutcome::Confirm(self.current_value()),
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = Some(title);
    }
}

impl ListSelector {
    /// The outcome of a navigation move: a live-preview emit for previewing selectors, else a redraw
    /// (`select-list.ts:103-108` `notifySelectionChange` → `onSelectionChange`).
    fn moved(&self) -> SelectorOutcome {
        if self.preview {
            SelectorOutcome::Preview(self.current_value())
        } else {
            SelectorOutcome::Redraw
        }
    }
}

/// The sentinel a [`CheckboxSelector`] confirm carries when **all** models are enabled (`enabledIds =
/// null`, `scoped-models-selector.ts:18`), distinct from an explicit ordered list. The run loop maps
/// this to "scope = full catalog".
pub const SCOPED_MODELS_ALL: &str = "*";

/// One catalog model in the scoped-models picker — upstream's `modelsById` entry.
#[derive(Clone, Debug)]
struct ModelRow {
    /// The model id (the `enabledIds` element, the confirm value **and** the row's primary text —
    /// `item.model?.id`, `scoped-models-selector.ts:249`).
    id: String,
    /// The model *name*, shown only in the `Model Name:` row (`:274`), never in the list rows.
    label: String,
    /// Provider id — the ` [provider]` badge (`:251`) and the `toggleProvider` grouping.
    provider: String,
}

/// The scoped-models checkbox + reorder selector (`scoped-models-selector.ts`, spec/tui/05 §6). Unlike
/// the plain [`ListSelector`], this renders the **full catalog** with per-row enable markers
/// (`✓`/`✗`), `Enter` **toggles** membership (it does *not* confirm), Alt+Up/Down **reorder** an
/// enabled model in cycle order, Ctrl+A/Ctrl+X enable/clear all, Ctrl+P toggles a whole provider, and
/// **Ctrl+S** confirms+persists. The `enabled` set mirrors Pi's `EnabledIds` (`None` = all enabled).
///
/// **This component owns its own row rendering.** Upstream's `updateList` (`:230-280`) adds bare
/// `Text` children — `prefix + id + " [provider]" + status` — it does **not** drive a `SelectList`,
/// so nothing here goes through [`SelectList`](crate::select_list::SelectList)'s padded two-column
/// layout. That is what put the enable marker in front of the label and the provider in a
/// right-aligned description column (S6/S7).
pub struct CheckboxSelector {
    /// The catalog, in catalog order — upstream's `modelsById` + `allIds` (`:93-94`).
    rows: Vec<ModelRow>,
    /// `None` = all enabled (no filter); `Some(ordered ids)` = the explicit cycle set, in order.
    enabled: Option<Vec<String>>,
    /// Highlighted index into the *filtered item* list (`selectedIndex`, `:97`).
    selected: usize,
    /// `maxVisible` — **8** here (`scoped-models-selector.ts:112`), not the 10 `/model` uses.
    max_visible: usize,
    /// The scoped-models bespoke bindings (Alt+Up/Down, Ctrl+A/X/P/S).
    models_keymap: ModelsKeymap,
    /// The shared `tui.select.*` bindings, so the footer can name the live confirm key
    /// (`keyText("tui.select.confirm")`, `:198`) instead of hardcoding `enter`.
    select_keymap: SelectKeymap,
    /// `isDirty` (`:113`): set by every mutation, cleared on save — drives the `(unsaved)` warning.
    dirty: bool,
    /// `config.refreshStatus` (`:149-152`): an optional `muted` `  {status}` row between the list
    /// spacer and the footer.
    refresh_status: Option<String>,
    /// The live search query — `this.searchInput` (`scoped-models-selector.ts:139`). **S5.**
    query: String,
    /// Caret byte offset within [`Self::query`].
    cursor: usize,
}

/// One built row — upstream's `ModelItem` (`scoped-models-selector.ts:68-72`). `model` is `None` for
/// an enabled id that is no longer in the catalog; upstream renders those ` [unavailable]` with a
/// dim `✗` (`:251`, `:258`) and counts them in the footer's `N unavailable`.
struct ModelItem {
    full_id: String,
    /// Index into [`CheckboxSelector::rows`], or `None` when the id is not in the catalog.
    model: Option<usize>,
    enabled: bool,
}

impl CheckboxSelector {
    /// Build from the full catalog `(id, name, provider, desc)` rows and the current scoped set
    /// (`None` = all enabled). The highlight preselects the first row.
    ///
    /// The fourth tuple element is **ignored**: upstream builds the row's badge from the model's own
    /// provider (`` ` [${item.model.provider}]` ``, `:251`), immediately after the id — there is no
    /// free-form description column in this component for it to land in.
    pub fn scoped_models(
        catalog: Vec<(String, String, String, Option<String>)>,
        enabled: Option<Vec<String>>,
    ) -> Self {
        let rows: Vec<ModelRow> = catalog
            .into_iter()
            .map(|(id, label, provider, _desc)| ModelRow { id, label, provider })
            .collect();
        CheckboxSelector {
            rows,
            enabled,
            selected: 0,
            max_visible: 8,
            models_keymap: ModelsKeymap::default(),
            select_keymap: SelectKeymap::default(),
            dirty: false,
            refresh_status: None,
            query: String::new(),
            cursor: 0,
        }
    }

    /// The live search query (test/inspection) — `getSearchInput().getValue()`.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The number of rows surviving the query (test/inspection) — `this.filteredItems.length`.
    pub fn visible_len(&self) -> usize {
        self.items().len()
    }

    /// Override the scoped-models bindings (JSON-configured `app.models.*`).
    pub fn set_models_keymap(&mut self, keymap: ModelsKeymap) {
        self.models_keymap = keymap;
    }

    /// Adopt the live `tui.select.*` bindings so the footer names the user's confirm key
    /// (`keyText("tui.select.confirm")`, `:198`) rather than the stock `enter`.
    pub fn set_select_keymap(&mut self, keymap: SelectKeymap) {
        self.select_keymap = keymap;
    }

    /// Set the optional catalog-refresh status row (`config.refreshStatus`, `:149-152`;
    /// `setRefreshStatus`, `:178-180`). An empty message clears it.
    pub fn set_refresh_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.refresh_status = if message.is_empty() { None } else { Some(message) };
    }

    /// The fuzzy search text for one catalog row — `getModelSearchText({id, provider, name})`
    /// (`model-search.ts:16-19`), the same provider-first shape [`crate::model_selector`] uses so a
    /// `provider/id` query ranks the way it does in `/model`.
    fn search_text(row: &ModelRow) -> String {
        format!("{p} {p}/{id} {p} {id} {name}", p = row.provider, id = row.id, name = row.label)
    }

    /// Insert a character at the caret (`Input.handleInput` printable arm).
    fn insert_char(&mut self, c: char) {
        self.query.insert(self.cursor, c);
        self.cursor = self.cursor.saturating_add(c.len_utf8());
    }

    /// Delete the character before the caret.
    fn backspace(&mut self) {
        let Some(ch) = self.query.get(..self.cursor).and_then(|s| s.chars().next_back()) else {
            return;
        };
        let start = self.cursor.saturating_sub(ch.len_utf8());
        self.query.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// `true` when model `id` is in the scoped set (`isEnabled`, `scoped-models-selector.ts:21`).
    fn is_enabled(&self, id: &str) -> bool {
        match &self.enabled {
            None => true,
            Some(list) => list.iter().any(|e| e == id),
        }
    }

    /// The current scoped set: `None` = all enabled, else the explicit ordered ids
    /// (test/inspection + confirm sourcing).
    pub fn enabled_ids(&self) -> Option<&[String]> {
        self.enabled.as_deref()
    }

    /// `getSortedIds` (`:62-66`): the enabled ids **first, in cycle order**, then every remaining
    /// catalog id. This is why Alt+↑/↓ is visible at all — the reorder shows up as the row moving.
    /// An enabled id that is no longer in the catalog stays in the list (upstream's
    /// `[...enabledIds, ...]` does not filter) and renders as `[unavailable]`.
    fn sorted_ids(&self) -> Vec<String> {
        match &self.enabled {
            None => self.rows.iter().map(|r| r.id.clone()).collect(),
            Some(en) => {
                let mut out: Vec<String> = en.clone();
                out.extend(
                    self.rows
                        .iter()
                        .filter(|r| !en.iter().any(|e| e == &r.id))
                        .map(|r| r.id.clone()),
                );
                out
            }
        }
    }

    /// `buildItems` + `refresh`'s `fuzzyFilter` (`:182-188`, `:211-224`): the sorted items, narrowed
    /// by the live query. Upstream falls back to the bare `fullId` as search text for an
    /// unavailable model (`:215-219`), so those stay searchable by id.
    fn items(&self) -> Vec<ModelItem> {
        let all: Vec<ModelItem> = self
            .sorted_ids()
            .into_iter()
            .map(|id| ModelItem {
                model: self.rows.iter().position(|r| r.id == id),
                enabled: self.is_enabled(&id),
                full_id: id,
            })
            .collect();
        if self.query.is_empty() {
            return all;
        }
        let texts: Vec<String> = all
            .iter()
            .map(|it| {
                it.model
                    .and_then(|i| self.rows.get(i))
                    .map_or_else(|| it.full_id.clone(), Self::search_text)
            })
            .collect();
        let matched = crate::fuzzy::filter(&texts, &self.query, String::as_str);
        let mut out = Vec::with_capacity(matched.len());
        for m in matched {
            if let Some(it) = all.get(m.index) {
                out.push(ModelItem {
                    full_id: it.full_id.clone(),
                    model: it.model,
                    enabled: it.enabled,
                });
            }
        }
        out
    }

    /// Clamp the highlight to the filtered length — `refresh`'s
    /// `Math.min(selectedIndex, max(0, filteredItems.length - 1))` (`:221`).
    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.visible_len().saturating_sub(1));
    }

    /// The highlighted model id, if any.
    fn current_id(&self) -> Option<String> {
        self.items().into_iter().nth(self.selected).map(|it| it.full_id)
    }

    /// Toggle membership of `id` (`toggle`, `:25-31`): from "all" the first toggle starts a set with
    /// only `id`; a member is removed; a non-member is appended.
    fn toggle(&mut self, id: &str) {
        self.enabled = match self.enabled.take() {
            None => Some(vec![id.to_string()]),
            Some(mut list) => {
                if let Some(pos) = list.iter().position(|e| e == id) {
                    list.remove(pos);
                } else {
                    list.push(id.to_string());
                }
                Some(list)
            }
        };
    }

    /// Move `id` by `delta` within the enabled order (`move`, `:50-60`). No-op (returning `false`)
    /// when all-enabled, when `id` is not a member, or when the move would leave the list — the
    /// three cases upstream's `:302-318` also treats as "nothing happened", so neither `isDirty` nor
    /// `selectedIndex` moves.
    fn reorder(&mut self, id: &str, delta: isize) -> bool {
        let Some(list) = self.enabled.as_mut() else { return false };
        let Some(idx) = list.iter().position(|e| e == id) else { return false };
        let new = idx as isize + delta;
        if new < 0 || new as usize >= list.len() {
            return false;
        }
        list.swap(idx, new as usize);
        true
    }

    /// Enable/clear every model of `id`'s provider (`toggleProvider`, `:354-368`): clear them if all
    /// are already enabled, else enable them all.
    fn toggle_provider(&mut self, id: &str) {
        let Some(provider) = self.rows.iter().find(|r| r.id == id).map(|r| r.provider.clone()) else {
            return;
        };
        let provider_ids: Vec<String> =
            self.rows.iter().filter(|r| r.provider == provider).map(|r| r.id.clone()).collect();
        let all_on = provider_ids.iter().all(|pid| self.is_enabled(pid));
        // Materialize the current set as an explicit list, then add/remove the provider's ids.
        let mut list: Vec<String> = match &self.enabled {
            None => self.rows.iter().map(|r| r.id.clone()).collect(),
            Some(l) => l.clone(),
        };
        if all_on {
            list.retain(|e| !provider_ids.contains(e));
        } else {
            for pid in provider_ids {
                if !list.contains(&pid) {
                    list.push(pid);
                }
            }
        }
        // Collapse back to "all" when every catalog model ended up enabled (Pi's null normalization).
        self.enabled = if list.len() == self.rows.len() { None } else { Some(list) };
    }

    /// The confirm value: [`SCOPED_MODELS_ALL`] when all are enabled, else the ordered ids joined by
    /// `\n` (the run loop splits this to rebuild the scoped set).
    fn confirm_value(&self) -> String {
        match &self.enabled {
            None => SCOPED_MODELS_ALL.to_string(),
            Some(list) => list.join("\n"),
        }
    }

    /// The title row: `theme.fg("accent", theme.bold("Model Configuration"))` at `paddingX = 0`
    /// (`:132`). **S7** — not `" Scoped Models"`; upstream's text is `Model Configuration` and it
    /// carries no leading space.
    fn title_line(theme: &UiTheme) -> Line<'static> {
        Line::from(Span::styled(
            "Model Configuration",
            theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD),
        ))
    }

    /// The subtitle row (`:133-135`): `muted` `Session-only. {keyText("app.models.save")} to save to
    /// settings.` — the guidance that explains why Enter does not close the dialog.
    fn subtitle_line(&self, theme: &UiTheme) -> Line<'static> {
        let save = self.models_keymap.keys_label(ModelsAction::Save).unwrap_or_default();
        Line::from(Span::styled(
            format!("Session-only. {save} to save to settings."),
            theme.muted_style(),
        ))
    }

    /// `getFooterText` (`:190-209`) — **S29**. Seven `·`-joined parts behind a two-space indent, the
    /// whole run `dim`, and when dirty a trailing space plus a `warning` `(unsaved)`.
    ///
    /// Every key comes from the live keymaps (`keyText`), never a literal: the `provider` toggle and
    /// the `N/M enabled` count were both missing entirely, and the indent was one column.
    fn footer_spans(&self, theme: &UiTheme) -> Vec<Span<'static>> {
        let k = |a: ModelsAction| self.models_keymap.keys_label(a).unwrap_or_default();
        let confirm = self.select_keymap.keys_label(SelectAction::Confirm).unwrap_or_default();
        // `countText` (`:191-196`): `enabledCount` counts only ids still in the catalog, and the
        // rest are reported as `N unavailable`.
        let count_text = match &self.enabled {
            None => "all enabled".to_string(),
            Some(en) => {
                let enabled_count =
                    en.iter().filter(|id| self.rows.iter().any(|r| &&r.id == id)).count();
                let unavailable = en.len().saturating_sub(enabled_count);
                let total = self.rows.len();
                if unavailable > 0 {
                    format!("{enabled_count}/{total} enabled · {unavailable} unavailable")
                } else {
                    format!("{enabled_count}/{total} enabled")
                }
            }
        };
        let parts = [
            format!("{confirm} toggle"),
            format!("{} all", k(ModelsAction::EnableAll)),
            format!("{} clear", k(ModelsAction::ClearAll)),
            format!("{} provider", k(ModelsAction::ToggleProvider)),
            format!("{}/{} reorder", k(ModelsAction::ReorderUp), k(ModelsAction::ReorderDown)),
            format!("{} save", k(ModelsAction::Save)),
            count_text,
        ];
        let joined = parts.join(" · ");
        if self.dirty {
            vec![
                Span::styled(format!("  {joined} "), theme.dim_style()),
                Span::styled("(unsaved)", theme.warning_style()),
            ]
        } else {
            vec![Span::styled(format!("  {joined}"), theme.dim_style())]
        }
    }

    /// `updateList` (`:230-280`) — the whole `listContainer`, in upstream's order.
    ///
    /// **S6.** The enable marker is *appended after* the id **and** the provider badge, and it is
    /// coloured: `theme.fg("success", " ✓")` / `theme.fg("dim", " ✗")` (`:252-258`). It is omitted
    /// entirely while every model is enabled (`allEnabled ? "" : …`). It was previously prepended
    /// into the label, uncoloured, which both shifted the id two columns right and lost the colour.
    ///
    /// **S7.** The provider is `theme.fg("muted", " [provider]")` immediately after the id (`:251`),
    /// and the highlighted model's *name* gets its own `Spacer(1)` + `  Model Name: …` row
    /// (`:269-279`) — the only place `label` is used.
    fn body_lines(&self, width: usize, theme: &UiTheme) -> Vec<Line<'static>> {
        let items = self.items();
        let mut lines: Vec<Line<'static>> = Vec::new();
        // `:233-236` — the empty case RETURNS, so no `Model Name:` row follows it.
        if items.is_empty() {
            lines.push(Line::from(Span::styled("  No matching models", theme.muted_style())));
            return lines;
        }
        let len = items.len();
        let start = self
            .selected
            .saturating_sub(self.max_visible / 2)
            .min(len.saturating_sub(self.max_visible));
        let end = (start + self.max_visible).min(len);
        let all_enabled = self.enabled.is_none();
        for (i, item) in items.iter().enumerate().take(end).skip(start) {
            let is_sel = i == self.selected;
            let row = item.model.and_then(|idx| self.rows.get(idx));
            let id = row.map_or(item.full_id.as_str(), |r| r.id.as_str());
            let mut spans: Vec<Span<'static>> = Vec::new();
            // `prefix` (`:248`): the `→ ` is accent, the unselected `  ` is a plain two-space pad.
            if is_sel {
                spans.push(Span::styled("→ ", theme.accent_style()));
                spans.push(Span::styled(id.to_string(), theme.accent_style()));
            } else {
                spans.push(Span::styled("  ", theme.base_style()));
                spans.push(Span::styled(id.to_string(), theme.base_style()));
            }
            spans.push(Span::styled(
                row.map_or_else(
                    || " [unavailable]".to_string(),
                    |r| format!(" [{}]", r.provider),
                ),
                theme.muted_style(),
            ));
            match (row, all_enabled, item.enabled) {
                (None, _, _) => spans.push(Span::styled(" ✗", theme.dim_style())),
                (Some(_), true, _) => {}
                (Some(_), false, true) => spans.push(Span::styled(" ✓", theme.success_style())),
                (Some(_), false, false) => spans.push(Span::styled(" ✗", theme.dim_style())),
            }
            lines.extend(crate::transcript::text_lines_of(&Line::from(spans), width, 0));
        }
        // Scroll indicator (`:263-267`).
        if start > 0 || end < len {
            lines.push(Line::from(Span::styled(
                format!("  ({}/{})", self.selected.saturating_add(1), len),
                theme.muted_style(),
            )));
        }
        // `Spacer(1)` + `  Model Name: {name}` for the highlighted item (`:269-279`).
        if let Some(item) = items.get(self.selected) {
            let text = match item.model.and_then(|idx| self.rows.get(idx)) {
                Some(r) => format!("  Model Name: {}", r.label),
                None => "  Model unavailable".to_string(),
            };
            lines.push(Line::from(""));
            lines.extend(crate::transcript::text_lines_of(
                &Line::from(Span::styled(text, theme.muted_style())),
                width,
                0,
            ));
        }
        lines
    }

    /// The complete natural render, top to bottom — the single source both
    /// [`Selector::desired_height`] and [`Selector::render`] read, so the measured height can never
    /// disagree with what is drawn.
    ///
    /// `ScopedModelsSelectorComponent`'s children (`scoped-models-selector.ts:130-156`):
    /// `DynamicBorder`(:130) · `Spacer`(:131) · title(:132) · subtitle(:133-135) · `Spacer`(:136) ·
    /// search `Input`(:140) · `Spacer`(:141) · listContainer(:145) · `Spacer`(:148) ·
    /// [refreshStatus(:150-151)] · footer(:154) · `DynamicBorder`(:156). **Four** spacers, and note
    /// this component — unlike `extension-selector.ts:74` — has NO spacer between its footer row and
    /// the bottom border.
    fn all_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let w = usize::from(width);
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(border_rule_line(width, theme));
        lines.push(Line::from(""));
        lines.push(Self::title_line(theme));
        lines.extend(crate::transcript::text_lines_of(&self.subtitle_line(theme), w, 0));
        lines.push(Line::from(""));
        // The `Input` is a bare container child (`:140`), so it renders at column 0 behind the
        // shared unstyled `"> "` prompt (S31, `input.ts:380`).
        lines.push(Line::from(input_line_spans(&self.query, self.cursor, theme)));
        lines.push(Line::from(""));
        lines.extend(self.body_lines(w, theme));
        lines.push(Line::from(""));
        if let Some(status) = &self.refresh_status {
            lines.extend(crate::transcript::text_lines_of(
                &Line::from(Span::styled(format!("  {status}"), theme.muted_style())),
                w,
                0,
            ));
        }
        lines.extend(crate::transcript::text_lines_of(
            &Line::from(self.footer_spans(theme)),
            w,
            0,
        ));
        lines.push(border_rule_line(width, theme));
        lines
    }
}

impl Selector for CheckboxSelector {
    fn desired_height(&self, width: u16) -> u16 {
        self.all_lines(width, &UiTheme::default()).len().min(usize::from(u16::MAX)) as u16
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // A `Vec<Line>` handed to a `Paragraph` draws `lines[0..area.height]` and drops the
        // TRAILING rows, so an over-tall dialog renders a strict PREFIX of this vector — exactly
        // what pi's layout engine does (`packages/tui/src/layout.ts:113,307-310`); see
        // `stack_rows`' doc for the full argument.
        let lines = self.all_lines(area.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Bespoke scoped-models bindings take precedence over the shared select map.
        if let Some(action) = self.models_keymap.action_for(key) {
            let Some(id) = self.current_id() else { return SelectorOutcome::Redraw };
            match action {
                // `:300-319` — a successful move also advances the highlight so it tracks the model
                // that moved, and only a successful move sets `isDirty`.
                ModelsAction::ReorderUp => {
                    if self.reorder(&id, -1) {
                        self.selected = self.selected.saturating_sub(1);
                        self.dirty = true;
                    }
                }
                ModelsAction::ReorderDown => {
                    if self.reorder(&id, 1) {
                        self.selected = self.selected.saturating_add(1);
                        self.dirty = true;
                    }
                }
                ModelsAction::EnableAll => {
                    self.enabled = None;
                    self.dirty = true;
                }
                ModelsAction::ClearAll => {
                    self.enabled = Some(Vec::new());
                    self.dirty = true;
                }
                ModelsAction::ToggleProvider => {
                    self.toggle_provider(&id);
                    self.dirty = true;
                }
                ModelsAction::Save => {
                    self.dirty = false;
                    return SelectorOutcome::Confirm(self.confirm_value());
                }
            }
            self.clamp_selection();
            return SelectorOutcome::Redraw;
        }
        // "Ctrl+C - clear search or cancel if empty" (`scoped-models-selector.ts:378-387`), tested
        // by `matchesKey(data, Key.ctrl("c"))` — a LITERAL upstream, not a `tui.select.*` id, which
        // is why this arm has to sit ahead of the generic `Cancel` below: cyrup's stock
        // `tui.select.cancel` binds `esc` AND `ctrl+c`, so routing Ctrl+C through `action_for`
        // first would close the dialog on the press that upstream spends clearing the query. Escape
        // is unconditional (`:390-392`) and stays with `Cancel`.
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !self.query.is_empty()
        {
            self.query.clear();
            self.cursor = 0;
            self.clamp_selection();
            return SelectorOutcome::Redraw;
        }
        match keymap.action_for(key) {
            // Up/Down WRAP (`:286-297`).
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                let len = self.visible_len();
                if len > 0 {
                    self.selected =
                        if self.selected == 0 { len.saturating_sub(1) } else { self.selected - 1 };
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                let len = self.visible_len();
                if len > 0 {
                    self.selected =
                        if self.selected.saturating_add(1) >= len { 0 } else { self.selected + 1 };
                }
                SelectorOutcome::Redraw
            }
            // Enter TOGGLES membership (it does NOT confirm) — `:322-331`.
            Some(SelectAction::Confirm) => {
                if let Some(id) = self.current_id() {
                    self.toggle(&id);
                    self.dirty = true;
                    self.clamp_selection();
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            // Everything else feeds the search `Input` (`:396-397`).
            None => {
                if let KeyCode::Char(c) = key.code
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.insert_char(c);
                    self.clamp_selection();
                    return SelectorOutcome::Redraw;
                }
                if key.code == KeyCode::Backspace {
                    self.backspace();
                    self.clamp_selection();
                    return SelectorOutcome::Redraw;
                }
                SelectorOutcome::Ignored
            }
        }
    }
}

/// A full-width `─` rule [`Line`] styled `border` — the [`Line`]-shaped twin of [`border_rule`], for
/// the envelopes that assemble a `Vec<Line>` instead of carving rects.
pub(crate) fn border_rule_line(width: u16, theme: &UiTheme) -> Line<'static> {
    Line::from(Span::styled("─".repeat(usize::from(width.max(1))), theme.border_style()))
}

/// A full-width `─` rule styled `border`, matching Pi's `DynamicBorder`
/// (`dynamic-border.ts:23` `color("─".repeat(max(1,width)))`) — **not** a ratatui `Block` border, so
/// it spans the whole inline width with no corners (spec/tui/05 §11).
fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    let rule = "─".repeat(width.max(1) as usize);
    Paragraph::new(Line::from(Span::styled(rule, theme.border_style())))
}
