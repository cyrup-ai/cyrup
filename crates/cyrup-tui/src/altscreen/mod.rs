//! The alternate-screen renderer tree, and the **renderer seam** both renderers satisfy —
//! cyrup's port of pi's `packages/tui/src/tui-alt-screen.ts` @v0.84.3 (ADR-0005 §Decision B).
//!
//! # What lives here today
//! ADR-0005 §B-2, §B-3, §B-4, §B-5, §B-8, §B-9, §B-10, §B-11, §B-12 and §B-13 — [`TuiRenderMode`] and
//! [`ViewportRenderer`], the seam pi spells `ViewportTUI` (`tui.ts:322-330`); the §B-3 alternate
//! screen itself, entered and torn back down by [`terminal`]; the §B-4 mouse and focus reporting
//! that screen asks the terminal for ([`mouse`]); the §B-5 scroll model over the retained document
//! and the scrollbar that reports it ([`scroll`]); the §B-8 text selection that capturing the mouse
//! obliges the renderer to provide, with the copy, link activation and paste a user does with one
//! ([`selection`]); the §B-9 viewport keybindings and their
//! fullscreen-only shadowing of the editor ([`keys`]); the §B-10 jump between prompts, over the
//! retained document's [`crate::Entry::User`]s rather than over pi's OSC 133 marks
//! ([`prompt_nav`]); the §B-11 flash stack ([`flash`]); the §B-12 inline-image lifecycle that
//! screen's ownership of every cell forces ([`images`]); and the §B-13 exit repaint that puts the
//! conversation into the user's scrollback on the way out, since nothing painted into the alternate
//! screen survives leaving it ([`exit`]).
//!
//! [`AltScreen`] — §B-3's renderer — is declared below and composes all of them, together with
//! [`document`]'s rendered-document bridge and its cache key and [`timers`]' deadline fold. It is
//! constructed from exactly one place: `App::install_renderer` (`app/mode_switch.rs`), which is
//! ADR-0005 §B-14's live mode switch. `App` holds it as an `Option`, `None` being regular mode.
//!
//! [`flash::FlashStack`] is owned by [`AltScreen`] and driven by [`ViewportRenderer::flash`], and
//! its expiry is folded into [`AltScreen::next_deadline`] — but nothing *pushes* to it yet: the
//! `/copy` fork that chooses between a flash and the status line
//! (`interactive-mode.ts:6106-6112`, §B-11) is `AppCommand::Copy`'s, in `app/execute*.rs`, and
//! would read [`crate::App::render_mode`] and call [`crate::App::renderer_mut`]. The inline half of
//! §B-11 is already true and unchanged: `App`'s `flash` is a no-op (`app/shell.rs:513-516`),
//! because the status line is what carries a transient notice in regular mode.
//!
//! The siblings that will join this module are the rendered-document cache and the scrollbar
//! painter §B-5 splits off once it owns their files, `document`/`scrollbar` — `terminal` (§B-3),
//! `mouse` (§B-4), `scroll` (§B-5), `wheel` (§B-6), `scrollbar_drag` (§B-7), `selection` (§B-8),
//! `keys` (§B-9), `prompt_nav` (§B-10), `flash` (§B-11), `images` (§B-12) and `exit` (§B-13) are the
//! eleven that have already arrived. §B-13's file is `exit`, not the `repaint` the ADR's file table
//! and [`terminal`]'s doc comments still name: the module is the second half of upstream's
//! `afterTerminalStop` (`tui-alt-screen.ts:311-333`) rather than a general repainter, and nothing
//! else in this tree is named for the escape sequence it happens to emit. §B-7's file is
//! `scrollbar_drag`, not the `drag` this list once named: §B-8
//! owns a second, unrelated pointer drag — text selection — and upstream keeps the two apart by
//! exactly that prefix (`scrollbarDrag` at `tui-alt-screen.ts:192` against `selectionDragPointer`
//! at `:188`).
//!
//! # The mouse dispatcher and what reaches it
//! [`AltScreen::handle_mouse`] is the `match` that offers a
//! [`ratatui::crossterm::event::MouseEvent`] to [`wheel::route`], [`scrollbar_drag::route`] and
//! [`selection::route`] in upstream's order (`tui-alt-screen.ts:564-575`). It is called from
//! `app/input.rs`'s `InputEvent::Mouse` arm; [`mouse::map_reader_event`] produces that event from a
//! crossterm report whenever reporting is armed, and answers `None` otherwise, which is what keeps
//! the inline renderer byte-identical to the unconditional discard it replaced.
//!
//! [`scrollbar_drag`] waits on that same dispatcher, and on the `AltUi` bag besides: its
//! [`scrollbar_drag::DragState`] is the live thumb grab pi holds as `scrollbarDrag`
//! (`tui-alt-screen.ts:192`), and it owns three of that dispatcher's arms:
//! [`scrollbar_drag::route`] after [`wheel::route`] and before §B-8's selection, which is
//! upstream's order and not a preference (`:565-575`); [`scrollbar_drag::update_hover`] from the
//! wheel path, where scrolling moves the thumb out from under a stationary pointer (`:685`); and
//! [`scrollbar_drag::cancel`] from the `FocusLost` arm (`:548-549`) and from both ends of the
//! alternate-screen excursion (`:260-261`, `:301-302`). All three are wired
//! ([`AltScreen::handle_mouse`] and [`AltScreen::handle_focus_lost`]).
//!
//! [`selection`] sits behind that dispatcher in five positions — three of them the same arms
//! [`scrollbar_drag`] names, immediately behind it. [`selection::route`] takes the press, drag
//! and release [`scrollbar_drag::route`] declined, which is upstream's `if (!handled)
//! this.handleSelectionMouseEvent(event)` (`tui-alt-screen.ts:575`); a report the scrollbar DID
//! claim calls [`selection::cancel`] instead, which is upstream clearing every selection field when
//! a thumb grab begins (`:776-784`). [`selection::focus_lost`] is the `FocusLost` arm beside
//! [`scrollbar_drag::cancel`] (`:543-559`), and it is a different clear on purpose — a completed
//! selection survives losing focus, only an in-flight one does not. [`selection::highlight`] paints
//! after the document and before [`flash::overlay`], upstream's composite order (`:1290`), and
//! [`selection::next_auto_scroll`] joins [`flash::next_expiry`] and [`scroll::next_hide`] in the
//! set of deadlines the loop arms its next wake on — it is what keeps a drag held against the
//! viewport edge extending with no further reports (`:949-951`).
//!
//! Two of its outcomes the dispatcher must finish itself, and neither is a gap in the unit:
//! [`selection::PointerOutcome::Copy`] carries the text because
//! [`crate::clipboard::copy_to_clipboard`] is `async` and the render path is not (§B-11 owns the
//! `await` and the `Copied!` / `Copy failed` flash, `interactive-mode.ts:6106-6112`), and
//! [`selection::PointerOutcome::Paste`] carries clipboard text because inserting it is the
//! [`crate::AppState`] editor's, which no renderer here may hold (rule 2 below).
//!
//! [`mouse::MouseSetup`] is owned by [`AltScreen`] too: armed by [`AltScreen::enter`] immediately
//! after `terminal::AltTerminal::enter` and disarmed by [`AltScreen::stop`] immediately before its
//! `leave`, which is where pi writes the same two sequences (`tui-alt-screen.ts:293`, `:306`).
//! Its reader-side half is already live and needs no owner:
//! `mouse::map_reader_event` is what `Event::Mouse` maps to in `app/input_reader.rs`, gated on a
//! process-global that only [`mouse::MouseSetup`] writes.
//!
//! [`keys::route`] has its position, and that position is the whole of the §B-9 shadowing rule
//! (`keybindings.ts:159`) — upstream's *placement*, not a flag: the alternate screen registers its
//! handler as an input listener (`tui-alt-screen.ts:227`) that runs before the focused component
//! (`tui.ts:834-848` against `tui.ts:892-897`), so a consumed `pageUp` never reaches the editor and
//! an unconsumed one is unaffected. `App::handle_input` (`app/input.rs`) offers every key to
//! [`AltScreen::handle_key`] immediately after its overlay block and before its selector block,
//! which is upstream's order down to the overlay deferral at `:538-540`/`:599`; a `false` — pi's
//! `undefined` — falls through to the selector, the global [`crate::Keymap`] and the editor
//! unchanged. The four unported `tui.altScreen.search*` arms (`:582-597`) have no counterpart at
//! all. The rule is nonetheless safe to wire in any order, because its second half is a
//! resolution-time mode test ([`crate::keymap::AltScreenKeymap::action_in_mode`]) that answers
//! nothing under [`TuiRenderMode::Regular`]: an inline `pageUp`, `home` or `end` keeps reaching the
//! editor and global maps whatever a caller does.
//!
//! [`prompt_nav`] is the tail of that same arm and the one piece here that waits on *two* things:
//! [`keys::route`] resolves `tui.altScreen.previousPrompt` / `nextPrompt` but does not perform them
//! — it reports [`keys::KeyOutcome::PreviousPrompt`] and [`keys::KeyOutcome::NextPrompt`], and
//! §B-3's dispatcher turns those into [`prompt_nav::previous`] / [`prompt_nav::next`], which is
//! upstream's `if (!isRelease) this.scrollToPrompt(-1)` / `(1)` (`tui-alt-screen.ts:629-636`). The
//! walk needs the *rendered* document besides — the retained [`crate::Entry`] list and the
//! entry-index-to-first-row map — and that is §B-5's per-frame cache, so both arrive as arguments
//! rather than as fields (rule 2 below). Nothing about the jump is mode-gated on its own account:
//! the binding that reaches it already is, by [`crate::keymap::AltScreenKeymap::action_in_mode`].
//!
//! [`images::ImageLifecycle`] is the fourth piece of ownerless state, and the one with the tightest
//! call-site contract: it is pi's `imageProtocol`/`savedCapabilities`/`uploadedKittyImages` triple
//! (`tui-alt-screen.ts:180-182`), and the renderer drives it from four positions, all of which
//! upstream fixes. [`images::ImageLifecycle::enter`] runs immediately after `terminal::AltTerminal::enter`
//! (`:264-270`); [`images::place`] paints the attachment strip on every frame — **instead of**
//! `crate::app::render_images`, which is the whole of the "no iterm2 image while the alt screen is
//! active" guarantee, since that painter is the crate's only emitter of a native graphics protocol;
//! [`images::ImageLifecycle::delete_all`] runs immediately before `terminal::TerminalSetup::leave`
//! and takes the same `preserve_screen` flag (`:306-308`); and
//! [`images::ImageLifecycle::restore`] runs after it (`:330-333`). The lifecycle is constructed in
//! [`AltScreen::build`], so those four positions are live; regular mode never constructs one, which
//! is what leaves the capability global, the transcript's image gate and the inline strip untouched
//! there.
//!
//! [`exit::repaint`]'s call site is not the renderer but [`terminal`]: it fills the row loop
//! `terminal::TerminalSetup::leave` marks in its `preserve_screen == false` branch, between the
//! `LeaveAlternateScreen` that has just restored the user's main screen and the `\x1b[0m` +
//! autowrap-back + `\r\n` that close the repaint — upstream's `:322-327` with the rows put back.
//! `AltTerminal::leave` and `TerminalSetup::leave` take the rendered document as a
//! `&[ratatui::text::Line<'static>]` parameter, the way [`prompt_nav`] takes the same cache rather
//! than holding it (rule 2 below), and [`AltScreen::stop`] threads it down.
//!
//! **That branch does not run today, and the reason is not this module's.** `preserve_screen` is
//! fixed at construction and the only construction site (`app/mode_switch.rs`) passes `true`, so
//! every teardown — the live mode switch AND session exit, which share `stop_fullscreen()` — takes
//! the `true` branch and skips the repaint. Making it reachable means deciding `preserve_screen`
//! per teardown rather than per renderer: `true` for a mode switch, where another renderer takes
//! the same terminal, and `false` for a real exit, where the transcript belongs in scrollback.
//! Filed as `ALT_SCREEN_EXIT_REPAINT_UNREACHABLE`.
//!
//! # Two structural rules this module holds its siblings to
//! Recorded here because they are what keeps twelve units out of one another's files, and because
//! both are compile errors rather than preferences:
//!
//! 1. **No `pub(super)` helper takes `&mut AltScreen<B>`.** A `Frame` exists only inside the
//!    closure `ratatui::Terminal::draw` hands out, and that closure already holds
//!    `&mut self.terminal` — so `self` cannot be passed in alongside it. The house idiom is to
//!    destructure first (`app/draw.rs:89`: `let App { terminal, state, .. } = self;`), which is why
//!    `AltScreen` splits into the terminal and one `AltUi` bag of renderer state, and why every
//!    painter takes `&mut AltUi` plus the narrow read-only refs it needs.
//! 2. **The renderer owns no application state.** No copy of the transcript, the theme or the
//!    keymap lives here: all three are [`crate::AppState`] fields, and a second copy of the theme
//!    would go stale the first time `App::set_theme` (`app/shell.rs:414-418`) ran — that setter writes
//!    `AppState` and nothing else. The alt-screen `draw`/`handle_input`
//!    entry points therefore take `&mut AppState`, and the *rendered* document is cached per frame
//!    so the [`ViewportRenderer`] methods — which get only `&mut self` — still have something to
//!    scroll over.

mod document;
mod exit;
mod flash;
mod images;
mod keys;
mod out;
/// Escape-capture handles, re-exported for `crate::tests` — `mod out` is private and the test
/// modules live outside this tree (`src/tests/mod.rs` explains the convention).
#[cfg(test)]
pub(crate) use out::{captured_text, Captured};
/// `pub(crate)`, unlike its siblings, for one item: `mouse::map_reader_event` is called from
/// `app/input_reader.rs`, which is outside this module tree (ADR-0005 §B-4). Everything else in it
/// stays `pub(super)` and reachable only from here.
pub(crate) mod mouse;
mod prompt_nav;
mod scroll;
mod scrollbar_drag;
mod selection;
mod terminal;
mod timers;
mod wheel;

/// The outcome of one pointer report, re-exported because `App::handle_input` matches on it —
/// ADR-0005 §B-8's `Copy`/`Paste` arms are performed by the app, not by the renderer.
pub use selection::PointerOutcome;

/// The `fullscreenScrollbar` policy (ADR-0005 §A-3), re-exported so the composition root can map
/// the setting onto it — `always` reserves the rightmost column, `auto` shows the bar only while
/// the content overflows and the view moved within 1000 ms, `hidden` never draws it.
pub use scroll::ScrollbarMode;

/// The pending-attachment strip §B-12 places, re-exported because the app owns the two
/// [`crate::AppState`] fields it borrows and builds it per frame.
pub use images::Strip;

use std::time::Duration;

use crate::component::Component;

/// Which renderer is live behind a [`ViewportRenderer`] — pi's `TuiBase.mode: TuiMode`
/// (`tui.ts:332`, over `type TuiMode = "regular" | "fullscreen"` at `tui.ts:284`), fixed per class:
/// `TuiMainScreen` declares `readonly mode = "regular"` (`tui-main-screen.ts:124`) and
/// `TuiAltScreen` declares `readonly mode = "fullscreen"` (`tui-alt-screen.ts:168`).
///
/// Two surfaces fork on it, and neither can be written until it exists: the `/copy` split between a
/// flash and a status-line notice — which upstream discriminates with
/// `this.ui instanceof TuiAltScreen` (`interactive-mode.ts:6108`, ADR-0005 §B-11) — and the
/// deliberate fullscreen-only shadowing of the unmodified editor bindings, `tui.altScreen.pageUp`
/// and `tui.altScreen.pageDown` defaulting to bare `pageUp`/`pageDown` under the comment at
/// `keybindings.ts:159` (§B-9).
///
/// Deliberately **not** `cyrup-config`'s `tuiMode` settings type (ADR-0005 §Decision A-3): that key
/// records what the user *asked for* and degrades any unknown value to `regular`
/// (`settings-manager.ts:1201-1203`); this records which renderer is actually *running*. The composition
/// root maps one onto the other, so this crate's seam does not wait on A-3 to be defined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TuiRenderMode {
    /// The inline (`Viewport::Inline`) renderer — pi's `"regular"`. cyrup's default in every build
    /// under every setting, exactly as upstream (`settings-manager.ts:1201-1203`).
    #[default]
    Regular,
    /// The alternate-screen renderer — pi's `"fullscreen"` (`tui-alt-screen.ts:168`).
    Fullscreen,
}

/// The seam both TUI renderers satisfy — pi's `ViewportTUI` (`tui.ts:322-330`).
///
/// Upstream splits the surface in two: every renderer implements `TUI`, and only the
/// alternate-screen renderer additionally implements `ViewportTUI` (`tui-alt-screen.ts:167`), which
/// the composition root discriminates with the `isViewportTUI` guard (`tui.ts:327-329`).
/// `TuiMainScreen` implements `TUI` alone (`tui-main-screen.ts:123`) and so has no
/// `setLayoutRoot`. cyrup collapses that into ONE trait, with [`ViewportRenderer::mode`] standing
/// in for `isViewportTUI` — which is also what makes the four operations below no-ops on the inline
/// renderer, upstream's shape rather than a shortcut: the main screen neither takes a layout root
/// nor scrolls, because the terminal's own scrollback does (R-ARCH-TUI-003).
///
/// # Scope: this is a RENDERER seam, not an application seam
/// Nothing in this crate holds the running app as a `Box<dyn ViewportRenderer>`, and no code may be
/// written on the assumption that it can. `App::run`, `App::handle_input`, `App::ingest_event` and
/// `App::state_mut` are not on this trait; `App::draw` carries a [`crate::RebuildBackend`] bound
/// (`app/draw.rs:6-8`) that `dyn` erases; and `ratatui::Terminal` exposes no consuming
/// accessor, so a backend cannot be moved out of one `Terminal` into another. ADR-0005 §B-14's live
/// mode switch is therefore **not** a pointer swap behind this trait: the app keeps its backend and
/// builds the second terminal with `RebuildBackend::rebuild()`, exactly as `resize_viewport`
/// already does (`app/draw.rs:113-118`). Object safety is kept because it costs nothing and lets a
/// future `/settings` row take `&mut dyn ViewportRenderer` — not because a swap depends on it.
///
/// Object-safe: no generic methods, no `Self`-returning methods, no associated types, no
/// `where Self: Sized`. [`crate::App`] stays generic over `B: ratatui::backend::Backend` and
/// implements this for every `B`.
///
/// No method returns a `Result`. Every operation is a state mutation the next frame reads:
/// scrolling past an edge is absorbed by the scroll view's clamp (`components/scroll-view.ts`)
/// rather than reported, and neither renderer performs I/O here — which is also what keeps the
/// implementations free of the `unwrap`/`expect` the workspace lints deny.
pub trait ViewportRenderer {
    /// Which renderer this is — cyrup's stand-in for pi's `isViewportTUI` guard
    /// (`tui.ts:327-329`) over the per-class `mode` field (`tui.ts:332`).
    fn mode(&self) -> TuiRenderMode;

    /// Install the component painted above the renderer's own default painting, replacing any
    /// previous root — pi's `setLayoutRoot(component: Component | undefined)` (`tui.ts:324`,
    /// implemented at `tui-alt-screen.ts:238-243`). `None` restores the default, matching
    /// upstream's `this.layoutRoot?.render(width) ?? super.render(width)`
    /// (`tui-alt-screen.ts:245-247`).
    ///
    /// Fullscreen installs the scroll view over the retained document (ADR-0005 §B-5); the editor,
    /// status band and selector slot are **not** part of the root — they are shared
    /// [`crate::AppState`] chrome that both renderers paint. The inline renderer stores nothing,
    /// which is upstream's shape: `TuiMainScreen` takes no layout root at all
    /// (`tui-main-screen.ts:123`).
    fn set_layout_root(&mut self, root: Option<Box<dyn Component>>);

    /// Scroll the viewport by `lines`, **negative for up** — pi's `scrollBy(lines: number)`
    /// (`tui-alt-screen.ts:397-400`), whose callers pass negatives to go up. Clamping, and the
    /// release of `follow: end` when the user scrolls away from the tail, belong to the
    /// implementation (`components/scroll-view.ts`).
    ///
    /// A no-op on the inline renderer, where native scrollback is what scrolls
    /// (R-ARCH-TUI-003).
    fn scroll_by(&mut self, lines: i32);

    /// Scroll to the first row of the document — pi's `scrollToTop` (`tui-alt-screen.ts:402-405`),
    /// delegating to the primary scroll view's `scrollToStart`. A no-op inline.
    fn scroll_to_top(&mut self);

    /// Scroll to the last row and re-arm `follow: end` — pi's `scrollToBottom`
    /// (`tui-alt-screen.ts:407-410`), delegating to the primary scroll view's `scrollToEnd`. A
    /// no-op inline.
    fn scroll_to_bottom(&mut self);

    /// Show a transient overlay message — pi's `flash(message: string, durationMs?: number)`
    /// (`tui-alt-screen.ts:534-536`), which delegates to `AltScreenFlashContainer.flash`
    /// (`components/alt-screen-flash.ts:22`). `None` selects upstream's `DEFAULT_DURATION_MS`
    /// of 1000 ms (`components/alt-screen-flash.ts:4`).
    ///
    /// A no-op on the inline renderer, where the status line already serves this purpose — which is
    /// exactly why `/copy` discriminates before calling this rather than calling it
    /// unconditionally: upstream branches on `this.ui instanceof TuiAltScreen` and falls back to
    /// `showStatus` (`interactive-mode.ts:6107-6112`), and [`ViewportRenderer::mode`] is what
    /// cyrup branches on instead (ADR-0005 §B-11).
    fn flash(&mut self, message: &str, duration: Option<Duration>);
}

// ---- the renderer -------------------------------------------------------------------------------

use std::time::Instant;

use ratatui::backend::Backend;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::error::TuiError;
use crate::image::ImageRenderer;
use crate::keymap::AltScreenKeymap;
use crate::theme::UiTheme;
use crate::transcript::{Entry, ImageOpts, TranscriptView};

/// The alternate-screen renderer — pi's `TuiAltScreen` (`tui-alt-screen.ts:167`).
///
/// This is the type ADR-0005 §Decision B is *about*: every unit B-3..B-13 contributes one sibling
/// module, and this struct is the only thing that composes them. It owns each sibling's state and
/// the fullscreen [`terminal::AltTerminal`], and every sibling entry point is reached from exactly
/// one method below — which is why the siblings never need to see each other and could be written
/// concurrently.
///
/// The document it paints is [`crate::TranscriptView`]'s retained document (ADR-0005 §B-1),
/// rendered to lines by the caller and handed over with [`Self::set_document`]. Rendering entries
/// needs the image options and output padding the app already owns for the inline path, so the
/// conversion stays there rather than being duplicated here.
pub struct AltScreen<B: Backend> {
    /// The fullscreen terminal and its armed restore (§B-3).
    term: terminal::AltTerminal<B>,
    /// The mouse reporting enabled on entry, disabled on the way out (§B-4).
    mouse: mouse::MouseSetup,
    theme: UiTheme,
    /// `this.layoutRoot` (`tui-alt-screen.ts:238-247`).
    layout_root: Option<Box<dyn Component>>,
    /// Scroll offset, follow-end and the viewport geometry (§B-5).
    scroll: scroll::ScrollState,
    /// Scrollbar policy and widget state (§B-5).
    bar: scroll::ScrollbarView,
    /// Thumb hover and drag (§B-7).
    drag: scrollbar_drag::DragState,
    /// Selection, clipboard and link activation (§B-8).
    selection: selection::SelectionState,
    /// Transient overlay messages (§B-11).
    flashes: flash::FlashStack,
    /// Inline-image placements and their eviction (§B-12).
    images: images::ImageLifecycle,
    /// The rendered document — one entry per row, `set_document`'s first argument.
    doc: Vec<Line<'static>>,
    /// Entry index → first rendered row, for §B-10's prompt walk.
    row_starts: Vec<usize>,
    /// What [`doc`](Self::doc) and [`row_starts`](Self::row_starts) were built from, as one
    /// comparable value — [`document::DocumentKey`]. `None` is "never built", which is why that
    /// type is deliberately not `Default`: an empty document at width 0 must not be mistaken for a
    /// build that already happened.
    ///
    /// The cache lives here rather than on [`crate::App`] because the rendered document already
    /// does: `set_document` takes the two vectors by value and this renderer keeps them for the
    /// whole time it is live. Holding the key beside them is what makes [`Self::sync_document`] a
    /// per-frame call — [`document::render_document`] re-renders every retained entry through
    /// markdown, syntax highlighting and image rasterisation, which is not.
    ///
    /// This is not a second copy of application state (rule 2 below): a key is nine scalars and a
    /// hash, it is rebuilt from [`crate::AppState`] on every frame, and nothing reads it as an
    /// answer about the app — only as an answer about this renderer's own cached rows.
    doc_key: Option<document::DocumentKey>,
    /// [`crate::TranscriptView::retained_dropped`] as of the build [`Self::doc`] holds.
    ///
    /// The front-trim delta [`document::rows_dropped`] needs is `retained_dropped - this`, and it
    /// has to be measured against the build being REPLACED — `row_starts[0]` of a freshly built
    /// document is `0` by construction, whatever was trimmed. `scroll::ScrollState` keeps
    /// the same counter for its own reconciliation, but privately and with no accessor, so this
    /// records the document's half of it: they answer different questions (how many rows have gone,
    /// against whether the offset has been shifted for them) and are advanced by the same call.
    doc_dropped: u64,
}

impl<B: Backend> AltScreen<B> {
    /// Enter the alternate screen and build the renderer — pi's `TuiAltScreen` construction plus
    /// `beforeTerminalStart` (`tui-alt-screen.ts:205-235`, `:257-295`).
    ///
    /// # Errors
    /// Propagates a terminal failure from [`terminal::AltTerminal::enter`] or from enabling mouse
    /// reporting. Either unwinds through the already-armed restore guard, so a failure here leaves
    /// the user on their original screen.
    pub(crate) fn enter(backend: B, theme: UiTheme) -> Result<Self, TuiError> {
        // One sink, cloned into each guard, so every escape this renderer emits lands in one
        // ordered stream. In production every clone is `io::stdout()`; under `for_test` they all
        // share a capture buffer.
        let out = out::Out::default();
        Self::build(backend, theme, out)
    }

    /// The shared body of [`Self::enter`] and (in test builds) `for_test`, parameterised only by
    /// where the escapes go.
    fn build(backend: B, theme: UiTheme, out: out::Out) -> Result<Self, TuiError> {
        let term = terminal::AltTerminal::enter(backend, out.clone())?;
        let mouse = mouse::MouseSetup::enable(out.clone())?;
        let images = images::ImageLifecycle::new(out.clone());
        Ok(Self {
            term,
            mouse,
            theme,
            layout_root: None,
            scroll: scroll::ScrollState::default(),
            bar: scroll::ScrollbarView::default(),
            drag: scrollbar_drag::DragState::default(),
            selection: selection::SelectionState::default(),
            flashes: flash::FlashStack::default(),
            images,
            doc: Vec::new(),
            row_starts: Vec::new(),
            doc_key: None,
            doc_dropped: 0,
        })
    }

    /// Build a renderer whose escapes are CAPTURED instead of written — the counterpart to pi's
    /// `new TuiAltScreen(new VirtualTerminal(w, h))` (`test/tui-alt-screen.test.ts:58-59`).
    ///
    /// The full [`Self::build`] path runs, terminal setup included, so the `?1049h`/`?7l`/`?25l`
    /// that `TerminalSetup::enter` emits and the mouse-mode string `MouseSetup::enable` emits are
    /// both in the returned buffer and assertable. Nothing reaches the real terminal, so this does
    /// NOT switch the `cargo test` process to the alternate screen.
    ///
    /// # Errors
    /// Propagates the same failures as [`Self::enter`]; with a capture sink neither write can fail,
    /// so in practice only `Terminal::new` can.
    #[cfg(test)]
    pub(crate) fn for_test(
        backend: B,
        theme: UiTheme,
    ) -> Result<(Self, out::Captured), TuiError> {
        let (sink, captured) = out::Out::capture();
        // Teardown tests pass their own `preserve_screen` to [`Self::stop`]; upstream's case
        // (`test/tui-alt-screen.test.ts:1336`) is the repainting one.
        let alt = Self::build(backend, theme, sink)?;
        Ok((alt, captured))
    }

    /// Hand over a document without a [`crate::TranscriptView`] — the `text.setText(...)` +
    /// `tui.requestRender()` pair upstream uses (`test/tui-alt-screen.test.ts:81-82`).
    ///
    /// [`Self::set_document`] exists to reconcile against a transcript's front-trim reports, which
    /// a fixture has none of; this is the same hand-over with that reconciliation skipped.
    #[cfg(test)]
    pub(crate) fn set_document_for_test(&mut self, lines: Vec<Line<'static>>, row_starts: Vec<usize>) {
        let rows = lines.len();
        self.doc = lines;
        self.row_starts = row_starts;
        // `draw` runs this every frame; doing it here too lets a test assert on `viewport_top` and
        // `max_scroll_top` before the first paint, as upstream does off `getViewport()`.
        let height = self.term.terminal_mut().size().map(|s| s.height).unwrap_or(0);
        scroll::update_layout(&mut self.scroll, rows, usize::from(height));
    }

    /// Park the viewport at an exact row — pi's `scrollView.scrollTo(row)` (`:420`), which is the
    /// landing [`prompt_nav`] performs and therefore the only way to set a walk up from a known
    /// offset. `scroll_by` moves by a delta and `scroll_to_top`/`scroll_to_bottom` reach only the
    /// ends, so neither can place the viewport between two prompts.
    #[cfg(test)]
    pub(crate) fn scroll_to_row_for_test(&mut self, row: usize) {
        scroll::scroll_to_row(&mut self.scroll, row);
    }

    /// The rendered cells, for viewport assertions — upstream's `terminal.getViewport()`
    /// (`test/virtual-terminal.ts:150`).
    #[cfg(test)]
    pub(crate) fn backend_for_test(&mut self) -> &B {
        self.term.terminal_mut().backend()
    }

    /// The largest legal scroll offset, for clamp assertions — [`scroll::max_scroll_top`].
    #[cfg(test)]
    pub(crate) fn max_scroll_top_for_test(&self) -> usize {
        scroll::max_scroll_top(&self.scroll)
    }

    /// The content width after the scrollbar's reservation — [`scroll::content_width`], pi's
    /// `getContentWidth` (`components/scroll-view.ts:86-88`): `always` narrows by one column,
    /// `auto` and `hidden` do not.
    #[cfg(test)]
    pub(crate) fn content_width_for_test(&self, width: u16) -> u16 {
        scroll::content_width(&self.bar, width)
    }

    /// The scrollbar policy in force — [`scroll::mode`], pi's `get scrollbar()`
    /// (`components/scroll-view.ts:67-69`).
    #[cfg(test)]
    pub(crate) fn scrollbar_mode_for_test(&self) -> ScrollbarMode {
        scroll::mode(&self.bar)
    }

    /// The negotiated image protocol — [`images::ImageLifecycle::protocol`], pi's `imageProtocol`
    /// (`tui-alt-screen.ts:180`).
    #[cfg(test)]
    pub(crate) fn image_protocol_for_test(&self) -> Option<crate::image::ImageProtocol> {
        self.images.protocol()
    }

    /// How many kitty uploads are retained — [`images::PlacementRegistry::tracked`], pi's
    /// `uploadedKittyImages.size` (`:1305`).
    #[cfg(test)]
    pub(crate) fn tracked_images_for_test(&self) -> usize {
        self.images.placements().tracked()
    }

    /// The first rendered row — pi's `TuiAltScreen.viewportTop` (`tui-alt-screen.ts:230-232`).
    ///
    /// `#[cfg(test)]` because it is an OBSERVATION POINT, not production surface: every one of
    /// upstream's ~25 references to it is in `test/tui-alt-screen.test.ts`. Gating it states that
    /// plainly and keeps the lib build free of a dead-code warning that would otherwise stand in
    /// for "a caller is missing" when none is.
    #[cfg(test)]
    pub(crate) fn viewport_top(&self) -> usize {
        scroll::scroll_top(&self.scroll)
    }

    /// Whether the view is pinned to the tail — pi's `TuiAltScreen.isFollowingOutput`
    /// (`tui-alt-screen.ts:234-236`), the second of upstream's two scroll observation points.
    ///
    /// `#[cfg(test)]` for the reason [`Self::viewport_top`] gives: upstream's only three references
    /// are all in its test file.
    #[cfg(test)]
    pub(crate) fn is_following_output(&self) -> bool {
        scroll::is_following_end(&self.scroll)
    }

    /// Hand over this frame's rendered document and its entry→row map, reconciling the scroll
    /// offset against any front-trim [`crate::TranscriptView::retained_dropped`] reports (§B-1/§B-5).
    ///
    /// The rows the trim removed are computed from the build being REPLACED — the `row_starts` and
    /// row count this renderer still holds at the moment of the call, plus how far
    /// `retained_dropped` has moved since that build ([`Self::doc_dropped`]). Deriving the number
    /// from the INCOMING map cannot work and is the silent mis-scroll R6 exists to prevent:
    /// `row_starts.first()` of a freshly built document is `0` by construction, so
    /// [`scroll::rebuild_rows`]'s `saturating_sub` would always be a no-op and a reader parked in
    /// history would jump forward on every [`crate::transcript::MAX_RETAINED_ENTRIES`] trim and on
    /// every [`crate::TranscriptView::clear_document`].
    pub(crate) fn set_document(
        &mut self,
        transcript: &TranscriptView,
        lines: Vec<Line<'static>>,
        row_starts: Vec<usize>,
    ) {
        let dropped = transcript.retained_dropped();
        let rows_dropped =
            document::rows_dropped(&self.row_starts, dropped.saturating_sub(self.doc_dropped), self.doc.len());
        self.doc_dropped = dropped;
        scroll::rebuild_rows(&mut self.scroll, dropped, rows_dropped);
        self.doc = lines;
        self.row_starts = row_starts;
    }

    /// The whole frame, in the fullscreen terminal's current geometry — the [`Rect`]
    /// [`Self::handle_mouse`] and [`Self::tick`] are addressed in.
    ///
    /// A zero-sized rect when the backend cannot answer, which every consumer already absorbs:
    /// [`scroll::content_width`] saturates, [`scroll::update_layout`] clamps, and a hit test against
    /// an empty rect matches nothing.
    pub(crate) fn area(&mut self) -> Rect {
        match self.term.terminal_mut().size() {
            Ok(size) => Rect::new(0, 0, size.width, size.height),
            Err(_) => Rect::new(0, 0, 0, 0),
        }
    }

    /// Rebuild the rendered document **only when this frame would produce a different one**, and
    /// hand it to [`Self::set_document`] — the §B-5 per-frame cache, driven from `App::draw`.
    ///
    /// `transcript`, `theme` and `images` are the [`crate::AppState`] the inline path already holds
    /// for its own commit flush (`app/draw.rs`), passed rather than owned (rule 2 below). The
    /// content width is this renderer's own — the frame width less the scrollbar column, upstream's
    /// `ScrollView.getContentWidth` (`components/scroll-view.ts:207-211`) — because it is a
    /// property of the viewport being painted and not of the app.
    ///
    /// The one thing that IS adopted is the theme, and it has to be: [`Self::theme`] is a projected
    /// copy and `App::set_theme` (`app/shell.rs:415-419`) writes [`crate::AppState`] and nothing
    /// else, so a `/theme` switch or a hot reload during a fullscreen session would otherwise paint
    /// the scrollbar and the layout root in the theme that was live when the screen was entered.
    /// Keyed on [`UiTheme::generation`], which is what that setter bumps.
    pub(crate) fn sync_document(
        &mut self,
        transcript: &TranscriptView,
        theme: &UiTheme,
        images: ImageOpts<'_>,
    ) {
        if self.theme.generation != theme.generation {
            self.theme = theme.clone();
        }
        let width = usize::from(self.content_width());
        let key = document::document_key(transcript, theme, width);
        if self.doc_key == Some(key) {
            return;
        }
        let (rows, row_starts) = document::render_document(
            transcript.document(),
            theme,
            width,
            transcript.output_pad(),
            images,
        );
        self.set_document(transcript, rows, row_starts);
        self.doc_key = Some(key);
    }

    /// The frame width less the scrollbar column — [`scroll::content_width`] over [`Self::area`].
    fn content_width(&mut self) -> u16 {
        let width = self.area().width;
        scroll::content_width(&self.bar, width)
    }

    /// The earliest instant this renderer wants the run loop to wake at, or `None` when nothing is
    /// pending — [`timers::next_deadline`] over the flash queue, the fading `auto` scrollbar and a
    /// selection drag held against the viewport edge.
    ///
    /// `None` is the loop's `std::future::pending()` arm, so an idle alternate screen costs no
    /// wakeups at all (`app/run.rs`).
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        timers::next_deadline(&self.flashes, &self.bar, &self.scroll, &self.selection)
    }

    /// Service whatever timer has come due, answering whether the frame is now stale —
    /// [`timers::tick`]. `true` **obliges** the caller to repaint: an expired flash is retired by
    /// [`flash::overlay`] on the paint, so ignoring it leaves the same elapsed deadline coming back
    /// from [`Self::next_deadline`] for ever.
    pub(crate) fn tick(&mut self, area: Rect) -> bool {
        timers::tick(
            &self.flashes,
            &self.bar,
            &mut self.selection,
            &mut self.scroll,
            &self.doc,
            area,
        )
    }

    /// Latch the terminal's image capabilities and suppress the protocols the alternate screen
    /// cannot own — §B-12, pi's `:267-269`.
    pub(crate) fn adopt_images(&mut self, transcript: &mut TranscriptView) {
        self.images.enter(transcript);
    }

    /// Hand the terminal's image capabilities and the transcript's graphics gate back — §B-12,
    /// pi's `:330-333`. Call AFTER [`Self::stop`], which is upstream's position.
    ///
    /// [`images::ImageLifecycle`]'s own `Drop` restores the process-global on the un-taken paths,
    /// but it cannot reach the transcript — so without this call an iterm2 session that entered
    /// fullscreen would come back to the inline renderer with `graphical_images` still `false` and
    /// every tool-result image stuck on its `[Image: …]` text branch for the rest of the session.
    pub(crate) fn restore_images(
        &mut self,
        transcript: &mut TranscriptView,
        renderer: &ImageRenderer,
    ) {
        self.images.restore(transcript, renderer);
    }

    /// Paint one frame in pi's z-order: document, selection highlight, scrollbar, flash overlay.
    ///
    /// # Errors
    /// Propagates a draw failure from the backend.
    ///
    /// `strip` is the pending-attachment strip the app owns (`AppState::image_renderer` +
    /// `pending_images`); `None` skips §B-12's placement pass entirely, which is what a frame with
    /// no attachments wants.
    pub(crate) fn draw(&mut self, strip: Option<images::Strip<'_>>) -> Result<(), TuiError> {
        // Destructured so the `draw` closure can borrow the renderer state while the terminal is
        // itself mutably borrowed — the shape `app/draw.rs:89` already uses.
        let Self { term, theme, layout_root, scroll, bar, selection, flashes, doc, images, .. } =
            self;
        term.terminal_mut()
            .draw(|frame: &mut Frame| {
                let area = frame.area();
                let content_width = scroll::content_width(bar, area.width);
                let viewport = Rect { width: content_width, ..area };
                scroll::update_layout(scroll, doc.len(), usize::from(area.height));

                let top = scroll::scroll_top(scroll);
                let visible: Vec<Line<'static>> =
                    doc.iter().skip(top).take(usize::from(area.height)).cloned().collect();
                frame.render_widget(Paragraph::new(Text::from(visible)), viewport);

                if let Some(root) = layout_root.as_mut() {
                    root.render(frame, viewport, theme);
                }
                selection::highlight(selection, scroll, doc, frame, viewport);
                // §B-12 — place the attachment strip and reconcile the registry, BEFORE the
                // scrollbar and the flash so neither is overpainted by a graphics escape.
                if let Some(strip) = strip.as_ref() {
                    images::place(images, frame, viewport, strip);
                }
                scroll::draw(bar, scroll, theme, frame, area);
                flash::overlay(flashes, frame, area);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }

    /// Route a key event. `false` means no `tui.altScreen.*` binding matched and the caller must
    /// offer the event onward to the editor — the fall-through §B-9's shadowing rule depends on.
    ///
    /// `entries` is [`crate::TranscriptView::document`], passed in rather than held so the renderer
    /// never owns a second copy of the transcript; §B-10's prompt walk is its only reader.
    /// Apply the `fullscreenScrollbar` setting (ADR-0005 §A-3 → §B-5). The composition root calls
    /// this after entering fullscreen; the default is `auto`, so a session that never sets it is
    /// already correct.
    pub(crate) fn set_scrollbar_mode(&mut self, mode: ScrollbarMode) {
        scroll::set_mode(&mut self.bar, &mut self.scroll, mode);
    }

    pub(crate) fn handle_key(
        &mut self,
        ev: &KeyEvent,
        keys: &AltScreenKeymap,
        entries: &[Entry],
    ) -> bool {
        match keys::route(&mut self.scroll, ev, keys, TuiRenderMode::Fullscreen) {
            keys::KeyOutcome::Pass => false,
            keys::KeyOutcome::Handled => true,
            keys::KeyOutcome::PreviousPrompt => {
                prompt_nav::previous(&mut self.scroll, entries, &self.row_starts);
                true
            }
            keys::KeyOutcome::NextPrompt => {
                prompt_nav::next(&mut self.scroll, entries, &self.row_starts);
                true
            }
        }
    }

    /// Route a mouse report in pi's fixed precedence — the scrollbar first (`:526-604`), then
    /// selection (`:605-963`), then the wheel. Returns the selection outcome the caller acts on.
    pub(crate) fn handle_mouse(&mut self, ev: &MouseEvent, area: Rect) -> selection::PointerOutcome {
        let content_width = scroll::content_width(&self.bar, area.width);
        let viewport = Rect { width: content_width, ..area };
        scrollbar_drag::update_hover(&mut self.bar, &mut self.scroll, ev.column, ev.row, area);
        if scrollbar_drag::route(&mut self.drag, &mut self.bar, &mut self.scroll, ev, area) {
            // A report the scrollbar CLAIMED clears every selection field, which is upstream
            // clearing them when a thumb grab begins (`tui-alt-screen.ts:776-784`) — the pointer
            // now belongs to the thumb, so an in-flight selection must end rather than be extended
            // by a gesture that is no longer over the document. This module's own doc has always
            // specified this arm; it was declared and left uncalled, which is what left
            // `selection::cancel` dead and the behaviour missing.
            selection::cancel(&mut self.selection);
            return selection::PointerOutcome::Handled;
        }
        match selection::route(&mut self.selection, &self.scroll, &self.doc, viewport, ev) {
            selection::PointerOutcome::Ignored => {
                if wheel::route(&mut self.scroll, viewport, ev) {
                    selection::PointerOutcome::Handled
                } else {
                    selection::PointerOutcome::Ignored
                }
            }
            other => other,
        }
    }

    /// A resize is this renderer's FULL REDRAW — pi's `fullRedraw` image arm
    /// (`tui-alt-screen.ts:1310-1316`).
    ///
    /// ratatui's `autoresize` throws away its previous buffer and repaints every cell, but kitty
    /// placements are written with graphics escapes that live outside that buffer model, so they
    /// survive at their old coordinates on top of the new layout unless they are explicitly
    /// unpinned. [`images::ImageLifecycle::clear_placements_for_redraw`] makes upstream's choice
    /// between retaining and freeing the uploads.
    pub(crate) fn handle_resize(&mut self) {
        self.images.clear_placements_for_redraw();
    }

    /// Focus loss cancels a live drag and a live selection — pi's `FOCUS_OUT` handling
    /// (`tui-alt-screen.ts:386-403`), and the reason §B-4 must enable `?1004h`.
    pub(crate) fn handle_focus_lost(&mut self) {
        scrollbar_drag::cancel(&mut self.drag, &mut self.bar, &mut self.scroll);
        let _ = selection::focus_lost(&mut self.selection);
    }

    /// Tear down in pi's order — `TuiBase.stop` (`tui.ts:752-762`): delete placements, repaint the
    /// document into the main screen unless this is a mode switch, disable mouse reporting, then
    /// leave the alternate screen.
    ///
    /// The flash queue is dropped first — pi's `dispose()` on the stop path
    /// (`tui-alt-screen.ts:303`, via `components/alt-screen-flash.ts:38-41`). Nothing repaints
    /// after this point, so it is bookkeeping rather than a visible step, but it keeps the
    /// teardown a faithful pair for [`ViewportRenderer::flash`]'s queue.
    pub(crate) fn stop(&mut self, preserve_screen: bool) {
        flash::clear(&mut self.flashes);
        self.images.delete_all(preserve_screen);
        let width = self.term.terminal_mut().size().map(|s| s.width).unwrap_or(0);
        self.mouse.disable();
        // pi's `dispose()` (`components/alt-screen-flash.ts:38-41`) on the way out
        // (`tui-alt-screen.ts:303`), so a notice queued in this excursion cannot survive into the
        // next one. The `:262` entry-side clear needs no call here: `enter` builds a fresh
        // `FlashStack::default()`, so an entering renderer starts empty by construction.
        flash::clear(&mut self.flashes);
        // `preserve_screen` is the CALLER's, not this renderer's: both of cyrup's teardowns need the
        // repaint, because the fullscreen frame path drains the transcript's pending queue and drops
        // it (`app/draw.rs`), so `self.doc` is the only surviving copy of the excursion's history.
        // Upstream reaches the same outcome by another route — `switchTuiMode` stops the alternate
        // screen with `preserveScreen: true` and lets the regular renderer re-render the shared chat
        // container (`interactive-mode.ts:833-840`) — which cyrup cannot do, because its committed
        // entries leave the app for the terminal's own scrollback. `true` remains correct for an
        // unwind, where there is no document to trust; `TerminalSetup::Drop` passes it.
        // The document goes to `leave`, not to a write before it: `exit::repaint` must land AFTER
        // `LeaveAlternateScreen` or the rows are painted onto the alternate screen and torn down
        // with it — pi writes them inside `afterTerminalStop` (`tui-alt-screen.ts:322-327`) for
        // exactly that reason.
        self.term.leave(preserve_screen, &self.doc, width);
    }

    /// The current selection's text, for ADR-0005 §B-11's `/copy` — upstream asks
    /// `getSelectionBounds() !== undefined` before choosing what to copy (`tui-alt-screen.ts:545`).
    ///
    /// `None` when nothing is selected, which is the caller's signal to fall back to the last
    /// assistant message.
    pub(crate) fn selection_text(&self) -> Option<String> {
        selection::has_selection(&self.selection)
            .then(|| selection::selected_text(&self.selection, &self.doc))
            .flatten()
    }
}

impl<B: Backend> ViewportRenderer for AltScreen<B> {
    fn mode(&self) -> TuiRenderMode {
        TuiRenderMode::Fullscreen
    }

    fn set_layout_root(&mut self, root: Option<Box<dyn Component>>) {
        self.layout_root = root;
    }

    fn scroll_by(&mut self, lines: i32) {
        scroll::scroll_by(&mut self.scroll, lines);
    }

    fn scroll_to_top(&mut self) {
        scroll::scroll_to_top(&mut self.scroll);
    }

    fn scroll_to_bottom(&mut self) {
        scroll::scroll_to_bottom(&mut self.scroll);
    }

    fn flash(&mut self, message: &str, duration: Option<Duration>) {
        flash::push(&mut self.flashes, message, duration);
    }
}
