//! Live TUI-mode switching — cyrup's port of pi's `InteractiveMode.switchTuiMode`
//! (`packages/coding-agent/src/modes/interactive/interactive-mode.ts:842-891` @v0.84.3),
//! ADR-0005 §Decision B-14.
//!
//! # What upstream carries across a swap, and why cyrup carries almost none of it
//! pi replaces one renderer *object* with another (`:862-874`), so everything the outgoing object
//! held has to be lifted out first and put back afterwards: the child components, the focused
//! component, the terminal, `showHardwareCursor`, `clearOnShrink`, `onDebug` and the main screen's
//! diff-render state (`:847-855`), then re-mounted, re-focused and re-started on the incoming one
//! (`:869-880`).
//!
//! cyrup's two renderers do not own any of that. The transcript, the editor, the status band, the
//! selector slot, the overlay stack, the theme and the keymaps are all [`AppState`] fields that
//! neither renderer may copy — the second structural rule the alternate-screen skeleton records
//! (`altscreen/mod.rs`, "the renderer owns no application state"), which exists precisely so a
//! swap has nothing to preserve. So there is no component list to move (pi `:847`, `:876`), no
//! focus to save and restore (`:848`, `:878`), no theme rebinding (`:881` —
//! `AppState::theme` is written by [`App::set_theme`] and read by whichever renderer is live), and
//! no extension input listeners to re-register (`:882` — cyrup reads terminal input in one place,
//! `app/input_reader.rs`, for both renderers). The retained document ADR-0005 §B-1 keeps
//! ([`crate::TranscriptView::document`]) is likewise *not* handed over: it is a transcript field
//! the incoming renderer reads in place.
//!
//! Its `retain_document` flag is the one piece of transcript state a switch does write, and only
//! because nothing else does: the document grows exclusively inside `drain_committed`
//! (`transcript/view.rs:110-116`), so an alternate screen entered with retention off has nothing to
//! paint for ever. [`App::enter_fullscreen`] turns it on and [`App::stop_fullscreen`] pairs the
//! turn-off with [`crate::TranscriptView::clear_document`], which is what keeps that field's "set
//! once, for the session's life" rule honest — see both methods for the whole argument.
//!
//! What is left is exactly one thing: the inline frame geometry [`App`] itself holds, which is
//! pi's `mainScreenRenderState` (`:853-855`, restored at `:871-873`). That is
//! [`MainScreenRenderState`] below.
//!
//! # The indirection that survives replacement
//! pi hands components a `Proxy` over "whatever renderer is current"
//! (`createInteractiveTuiReference`, `:391-419`) so a held reference keeps working after the swap.
//! cyrup needs no proxy — nothing in this crate stores a renderer reference — but it needs the
//! same *single point of resolution*, so that the units which fork on the live renderer (the
//! ADR-0005 §B-11 `/copy` flash-vs-status-line split, §B-9's mode-gated key resolution) read it in
//! one place instead of thirteen. [`App::render_mode`] and [`App::renderer_mut`] are that point.
//!
//! # What still has no caller
//! [`App::switch_tui_mode`] is complete in both directions — ADR-0005 §B-3's `AltScreen` is
//! constructed by [`App::install_renderer`] and taken back down by [`App::stop_fullscreen`] — but
//! nothing in this crate *calls* it, and the two callers upstream has are both outside `cyrup-tui`:
//!
//! 1. **The composition root.** `crates/cyrup/src/interactive.rs` is where `--tui-mode`
//!    (`crates/cyrup/src/cli/args.rs:185`) and the persisted `settings.tuiMode`
//!    (`cyrup_config::settings::TuiMode`, ADR-0005 §A-3) are merged and turned into one
//!    [`TuiRenderMode`] for the boot switch. Note the two `TuiMode` enums it has to reconcile — the
//!    clap `ValueEnum` at `crates/cyrup/src/cli/enums.rs:79` and the settings value — carry the
//!    same two variants and the same lowercase spellings, so the mapping is total.
//! 2. **The `/settings` rows.** pi's `tui-mode` and `fullscreen-scrollbar`
//!    (`components/settings-selector.ts:671-676` and `:685-691`, dispatched at `:904-905` /
//!    `:910-911`) live in `app/settings_rows.rs`. The caller shape they need is complete here:
//!    [`ModeSwitch::refusal_status`] is the string pi shows on a refused switch (`:4729`), and a
//!    successful switch is pi's `TUI mode: ${mode}` receipt (`:4734`). `fullscreen-scrollbar` needs
//!    one more seam that does not exist yet: `altscreen::scroll::set_mode` and its `ScrollbarMode`
//!    are `pub(super)`, so `AltScreen` has no public setter for the policy §A-3's
//!    `FullscreenScrollbar` records.

use super::*;

use crate::altscreen::{TuiRenderMode, ViewportRenderer};

/// The inline renderer's frame geometry, lifted across a live mode switch — pi's
/// `TuiMainScreenRenderState` (`packages/tui/src/tui-main-screen.ts:112-120`), captured at
/// `interactive-mode.ts:853-855` and put back at `:871-873`.
///
/// Upstream's record is its own cell-diff bookkeeping (`previousLines`, `previousWidth`,
/// `cursorRow`, …) because `TuiMainScreen` diffs the screen itself. cyrup does not: `ratatui`'s
/// `Terminal` owns the front/back buffers, and a swap destroys that `Terminal` along with them.
/// The only inline render state that lives on [`App`] — and would therefore be silently lost — is
/// the pair below, both of which [`App::draw`] reads on every frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MainScreenRenderState {
    /// [`App`]'s `viewport_height` at capture time: the inline viewport's content height, i.e. the
    /// `Viewport::Inline(h)` the live `Terminal` was constructed with.
    ///
    /// Recorded for the same reason pi records `previousHeight`, but **not** written back by
    /// [`App::restore_main_screen_render_state`] — see that method for why.
    pub viewport_height: u16,
    /// [`App`]'s `live_floor` at capture time: the grow-only high-water mark for the live region
    /// while a turn is streaming. This is the field a mid-turn switch would otherwise lose, and
    /// losing it makes the returning inline region collapse to the idle editor and grow back on the
    /// next event — the per-tool FLICKER that floor exists to prevent.
    pub live_floor: u16,
}

/// The outcome of [`App::switch_tui_mode`] — pi's `switchTuiMode(...): boolean`
/// (`interactive-mode.ts:842`), widened from a bare `true`/`false`.
///
/// `[CYRUP-DELTA]`: upstream's caller can afford one bit because it has exactly one refusal to
/// report — "close your overlays" (`:4727-4731`). cyrup has two refusals, and they need different
/// words: an overlay is the user's to dismiss, while a renderer this build does not contain is not.
/// [`ModeSwitch::accepted`] is the bit pi actually branches on, so the caller shape stays
/// upstream's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeSwitch {
    /// The requested mode is already the live one; nothing was touched — pi's
    /// `if (mode === previousUi.mode) return true` (`:844`).
    Unchanged,
    /// The renderer was replaced — pi's `return true` at `:890`.
    Switched,
    /// Refused because an overlay is up — pi's `if (previousUi.hasOverlayEntries) return false`
    /// (`:845`), over [`AppState::overlays`], this crate's floating z-stack.
    ///
    /// Upstream refuses rather than tearing overlays down because an overlay is mounted on the
    /// renderer it would be swapped out from under; cyrup's overlays are [`AppState`]'s and would
    /// technically survive, but the refusal is kept: the alternate screen re-lays every overlay out
    /// against a full-screen viewport, and doing that underneath an open dialog moves the thing the
    /// user is currently answering. The one upstream path that *does* switch with overlays up
    /// dismisses them first and explicitly (`:835-836`).
    BlockedByOverlay,
    /// Refused because the target renderer could not be started — [`crate::AltScreen::enter`]
    /// failed to enter the alternate screen or to arm mouse reporting on this terminal.
    ///
    /// `[CYRUP-DELTA]`: upstream has no such state, because `TuiAltScreen` construction cannot
    /// fail — its writes are fire-and-forget on a `WriteStream`. It is reported rather than papered
    /// over so a caller never records a `tuiMode` the session is not actually running. The failure
    /// unwinds through the restore guards `enter` arms before its first byte, so the user is left
    /// on their original screen and the inline renderer is still the live one.
    RendererUnavailable,
}

impl ModeSwitch {
    /// The single bit pi's caller branches on: `true` for everything that leaves the app running
    /// the requested mode, `false` for a refusal (`interactive-mode.ts:4727`).
    #[must_use]
    pub fn accepted(self) -> bool {
        matches!(self, ModeSwitch::Unchanged | ModeSwitch::Switched)
    }

    /// The status-line text a refusal should show, or `None` when the switch was accepted.
    ///
    /// The overlay wording is upstream's verbatim (`interactive-mode.ts:4729`). The second string
    /// has no upstream counterpart — see [`ModeSwitch::RendererUnavailable`].
    ///
    /// A refusal also obliges the caller to put the `/settings` row back to the mode that is
    /// actually live (pi `updateValue("tui-mode", this.ui.mode)`, `:4728`), which is what
    /// [`App::render_mode`] answers.
    #[must_use]
    pub fn refusal_status(self) -> Option<&'static str> {
        match self {
            ModeSwitch::Unchanged | ModeSwitch::Switched => None,
            ModeSwitch::BlockedByOverlay => Some("Close active overlays before changing TUI mode"),
            ModeSwitch::RendererUnavailable => {
                Some("Fullscreen TUI mode is not available in this build")
            }
        }
    }
}

/// The two optional arguments of pi's `switchTuiMode(mode, restoreProgress = true,
/// startRenderer = true)` (`interactive-mode.ts:842`), as a struct rather than defaulted
/// parameters.
///
/// [`Default`] is upstream's default pair, so [`App::switch_tui_mode`] with
/// `ModeSwitchOptions::default()` is upstream's ordinary `/settings` call (`:4727`). The one call
/// site that passes anything else is the fullscreen *exit* path, which switches back to inline
/// purely to reprint the transcript on the main screen and therefore wants neither the renderer
/// started nor the progress indicator re-armed (`switchTuiMode("regular", false, false)`, `:836`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSwitchOptions {
    /// pi's `restoreProgress`: after the swap, re-arm the OSC 9;4 taskbar indicator if the session
    /// is still busy (`:883-889`). See [`App::restore_terminal_progress_after_switch`].
    pub restore_progress: bool,
    /// pi's `startRenderer`: run the post-install half of the swap at all (`:879-889`). `false`
    /// installs the renderer and stops, which is what the exit path at `:836` wants — it is about
    /// to leave the alternate screen for good and only needs the inline renderer *selected*, not
    /// running.
    pub start_renderer: bool,
}

impl Default for ModeSwitchOptions {
    /// pi's declared defaults — `restoreProgress = true, startRenderer = true`
    /// (`interactive-mode.ts:842`). Not derivable: `bool`'s own default is the opposite of both.
    fn default() -> Self {
        ModeSwitchOptions { restore_progress: true, start_renderer: true }
    }
}

impl<B: Backend> App<B> {
    /// Which renderer is live — pi's `this.ui.mode` (`interactive-mode.ts:4728`, `:834`).
    ///
    /// The single read point ADR-0005 §B-14 owes the units that fork on it, so that the two
    /// surfaces that branch on the live renderer — §B-11's `/copy` flash-vs-status-line split and
    /// §B-9's mode-gated key resolution — read it here instead of testing the field.
    ///
    /// Delegates to the §B-2 seam in both arms rather than answering from a field of its own,
    /// because the seam is where the answer is *defined*: each renderer fixes its own `mode` on the
    /// type, exactly as upstream does (`tui-main-screen.ts:124` against `tui-alt-screen.ts:168`).
    #[must_use]
    pub fn render_mode(&self) -> TuiRenderMode {
        match self.altscreen {
            Some(ref alt) => ViewportRenderer::mode(alt),
            None => <Self as ViewportRenderer>::mode(self),
        }
    }

    /// The live renderer behind the ADR-0005 §B-2 seam — cyrup's answer to pi's stable-reference
    /// `Proxy` (`createInteractiveTuiReference`, `interactive-mode.ts:391-419`, installed at `:585`).
    ///
    /// Every renderer-directed operation in `app/` should go through here rather than calling
    /// [`ViewportRenderer`] methods on `self` directly: the point of the indirection is that the
    /// receiver changes when the mode does, and a direct call would keep addressing the inline
    /// renderer after a switch. It resolves to the alternate screen whenever one is installed and
    /// to `self` — the inline renderer, whose four scroll/flash operations are no-ops — otherwise.
    ///
    /// A trait object rather than a generic is deliberate and costs nothing: [`ViewportRenderer`]
    /// is object-safe by construction and the five operations it exposes are one state mutation
    /// each, never a hot path.
    ///
    /// A `match` on the field itself rather than on `self.altscreen.as_mut()`: the `as_mut` form
    /// is the borrow pattern NLL cannot accept, because the `&mut` it returns carries this
    /// function's own lifetime and so conflicts with the `self` the `None` arm hands back. A `ref
    /// mut` binding on the place creates that borrow inside the `Some` arm only.
    pub fn renderer_mut(&mut self) -> &mut dyn ViewportRenderer {
        match self.altscreen {
            Some(ref mut alt) => alt as &mut dyn ViewportRenderer,
            None => self,
        }
    }

    /// Apply the `fullscreenScrollbar` setting to the live alternate screen (ADR-0005 §A-3 → §B-5).
    /// A no-op inline, where there is no bar to configure.
    pub fn set_fullscreen_scrollbar(&mut self, mode: crate::altscreen::ScrollbarMode) {
        if let Some(alt) = self.altscreen.as_mut() {
            alt.set_scrollbar_mode(mode);
        }
    }

    /// Lift the inline frame geometry out of [`App`] before a swap — pi's
    /// `previousUi.captureRenderState()` (`interactive-mode.ts:853-855`, over
    /// `tui-main-screen.ts:134-144`).
    #[must_use]
    pub fn capture_main_screen_render_state(&self) -> MainScreenRenderState {
        MainScreenRenderState {
            viewport_height: self.viewport_height,
            live_floor: self.live_floor,
        }
    }

    /// Put the inline frame geometry back after a swap **to** regular mode — pi's
    /// `nextUi.restoreRenderState(this.mainScreenRenderState)` (`interactive-mode.ts:871-873`, over
    /// `tui-main-screen.ts:146-155`).
    ///
    /// [`MainScreenRenderState::live_floor`] is restored verbatim: it describes the *turn*, which
    /// the excursion did not interrupt.
    ///
    /// [`MainScreenRenderState::viewport_height`] is deliberately **not** restored — it is seeded
    /// to `0` instead, exactly as `App::new` seeds it. That field describes cells the alternate
    /// screen has since owned and repainted, and [`App::draw`] uses it twice in ways that would
    /// both be wrong with a stale value: it rebuilds the viewport only when the desired height
    /// *differs* from it (so restoring the old height can suppress the rebuild that re-anchors the
    /// inline region at the bottom of the screen), and it hands it to
    /// [`RebuildBackend::reanchor_inline`] as the height of the region to erase (so a stale value
    /// erases rows the returning renderer never wrote). Seeding `0` forces the rebuild and erases
    /// nothing, which is what a renderer arriving at an unknown screen must do.
    ///
    /// This asymmetry is upstream's too, in its own currency: `restoreRenderState` blanks every
    /// image line out of `previousLines` and drops the whole `previousKittyImageIds` set
    /// (`tui-main-screen.ts:147-148`) for the same reason — the parts of the captured state that
    /// describe cells the other renderer has since overwritten must not be believed.
    pub fn restore_main_screen_render_state(&mut self, state: MainScreenRenderState) {
        self.live_floor = state.live_floor;
        self.viewport_height = 0;
    }

    /// Re-arm the OSC 9;4 taskbar progress indicator if the session is still busy — pi's
    /// `if (restoreProgress && getShowTerminalProgress() && (isStreaming || isCompacting))
    /// terminal.setProgress(true)` (`interactive-mode.ts:883-889`).
    ///
    /// Upstream needs this because `previousUi.stop()` (`:857`) takes the terminal down with the
    /// renderer, clearing the indicator; the same is true here, since leaving and re-entering the
    /// screen tears down and rebuilds the terminal the sequence was written to.
    ///
    /// The `getShowTerminalProgress()` half of upstream's condition is not repeated: it is already
    /// [`crate::TerminalProgress::set`]'s own gate, which answers `None` and records nothing when
    /// the `terminal.showTerminalProgress` row is off. The write itself is parked for the run loop
    /// rather than emitted here — the split [`App::flush_terminal_progress`] documents — so this
    /// stays a pure state transition like every other step of the swap.
    ///
    /// Returns whether a re-arm was recorded.
    pub fn restore_terminal_progress_after_switch(&mut self) -> bool {
        // pi reads `session.isStreaming || session.isCompacting`. cyrup's session-event fold keeps
        // both on `AppState`: the streaming half is the status band's own flag (set on `AgentStart`,
        // cleared on `AgentEnd`, so it spans the whole multi-step turn), and the compacting half is
        // the working indicator being live with `IndicatorKind::Compaction` — the same pair the
        // four `TerminalProgress::set` call sites in the fold are driven by.
        let busy = self.state.status.streaming
            || (self.state.indicator.is_active()
                && self.state.indicator.kind() == IndicatorKind::Compaction);
        if !busy {
            return false;
        }
        self.state.terminal_progress.set(true).is_some()
    }
}

/// The half of §B-14 that constructs a renderer, and therefore the half that needs a backend it can
/// take a second handle to.
///
/// The bound is [`RebuildBackend`] for the reason [`crate::ViewportRenderer`]'s scope note gives and
/// `App::draw` already demonstrates (`app/draw.rs:6-9`): `ratatui::Terminal` has no consuming
/// accessor, so the fullscreen terminal is built over `self.terminal.backend().rebuild()` rather
/// than by moving the backend across. Splitting the impl rather than widening the whole type's
/// bound keeps every `App<B: Backend>` — a `TestBackend` app included — able to read
/// [`App::render_mode`] and hold the state above.
impl<B: RebuildBackend> App<B> {
    /// Switch the live renderer — pi's `switchTuiMode` (`interactive-mode.ts:842-891`).
    ///
    /// The protocol, in upstream's order: refuse nothing when the mode is already live (`:844`);
    /// refuse a switch made underneath an open overlay (`:845`); capture the outgoing renderer's
    /// frame geometry (`:853-855`); install the incoming renderer (`:862-875`); and — unless the
    /// caller asked for the selection only (`:879`) — finish the swap (`:880-889`).
    ///
    /// Everything upstream does between `:847` and `:878` that is *not* in that list is absent
    /// because cyrup has nothing for it to move; the module doc accounts for each of those pieces
    /// of state. `nextUi.invalidate()` (`:877`) is likewise structural here rather than a call:
    /// the transcript's render cache keys on `(generation, width, theme generation)`, and a swap
    /// that changes the content width invalidates it by that key alone.
    ///
    /// Recording the new mode in the persisted settings (pi `:875`, `:4732`) is the *caller's*,
    /// exactly as it is upstream. ADR-0005 §A-3 has landed, so the keys now exist:
    /// `cyrup_config::settings::TuiMode` and `SettingsManager::set_tui_mode`.
    #[must_use]
    pub fn switch_tui_mode(&mut self, mode: TuiRenderMode, opts: ModeSwitchOptions) -> ModeSwitch {
        if mode == self.render_mode() {
            return ModeSwitch::Unchanged;
        }
        if !self.state.overlays.is_empty() {
            return ModeSwitch::BlockedByOverlay;
        }
        // Captured BEFORE the install, because it is the OUTGOING renderer's state — upstream's
        // ordering at `:853-855` against `:871-873`, and the reason it is a value rather than a
        // borrow.
        let saved = self.capture_main_screen_render_state();
        if !self.install_renderer(mode, saved) {
            return ModeSwitch::RendererUnavailable;
        }
        // pi `:879` — `if (!startRenderer) return true`. The renderer is selected; nothing that
        // touches the terminal runs.
        if !opts.start_renderer {
            return ModeSwitch::Switched;
        }
        if opts.restore_progress {
            self.restore_terminal_progress_after_switch();
        }
        ModeSwitch::Switched
    }

    /// Make `mode` the live renderer, returning whether it could be — the cyrup half of pi's
    /// `createInteractiveTui` + `this.renderer = nextUi` (`interactive-mode.ts:862-874`, over the
    /// composition root at `:368-388`).
    ///
    /// Split out of [`Self::switch_tui_mode`] because it is the only step that constructs anything:
    /// everything around it is a pure state transition, and this is where the alternate screen is
    /// entered and left.
    fn install_renderer(&mut self, mode: TuiRenderMode, saved: MainScreenRenderState) -> bool {
        match mode {
            TuiRenderMode::Fullscreen => self.enter_fullscreen(),
            // Coming back inline there is nothing to construct: `App` IS the inline renderer, and
            // it never stopped being one — which is upstream's shape too, where the only work the
            // main screen's arrival needs is `restoreRenderState` (`:871-873`). What there IS to do
            // is take the alternate screen down, which is [`Self::stop_fullscreen`]'s whole body.
            TuiRenderMode::Regular => {
                // `false`: the excursion's history has to land in native scrollback before the inline
                // renderer resumes below it. Upstream can pass `preserveScreen: true` here
                // (`interactive-mode.ts:857`) because its regular renderer re-renders the shared chat
                // container; cyrup's committed entries have already left the app for the terminal, so
                // the repaint is the only thing that carries them across.
                self.stop_fullscreen(false);
                self.restore_main_screen_render_state(saved);
                true
            }
        }
    }

    /// Build §B-3's [`AltScreen`] over a fresh handle to this app's own backend and make it the
    /// live renderer — the `Fullscreen` half of [`Self::install_renderer`].
    ///
    /// The backend comes from [`RebuildBackend::rebuild`], not from a move: `ratatui::Terminal`
    /// exposes no consuming accessor, so the second terminal is built over a second handle to the
    /// same tty exactly as `App::draw`'s `resize_viewport` builds the resized inline one
    /// (`app/draw.rs:113-118`). `App`'s own `Terminal` is left alone for the whole excursion and
    /// resumes when [`Self::stop_fullscreen`] drops the alternate screen — which is why
    /// [`Self::restore_main_screen_render_state`] seeds `viewport_height` back to `0` rather than
    /// believing the height it had before.
    ///
    /// `false` — [`ModeSwitch::RendererUnavailable`] — on any terminal failure from
    /// [`AltScreen::enter`], and refusing is the correct answer for the reason the state's own doc
    /// gives: a half-entered alternate screen with no renderer painting it is a blank screen with
    /// no way back. `enter` unwinds through its own already-armed restore guards, so a failure here
    /// leaves the user on their original screen.
    fn enter_fullscreen(&mut self) -> bool {
        let backend = self.terminal.backend().rebuild();
        let theme = self.state.theme.clone();
        let Ok(alt) = AltScreen::enter(backend, theme) else {
            return false;
        };
        self.adopt_fullscreen_renderer(alt);
        true
    }

    /// [`Self::enter_fullscreen`] over a capture sink — the `App`-level twin of
    /// [`crate::AltScreen::for_test`], and the only way a test can reach the fullscreen frame path
    /// without switching the `cargo test` process to the alternate screen.
    ///
    /// It exists because the teardown's failure mode is an INTERACTION, not a unit: `draw_fullscreen`
    /// drains the transcript's pending queue and drops it (`app/draw.rs`), and [`Self::stop_fullscreen`]
    /// clears the retained document one line after `stop`. Each is locally correct; together they once
    /// destroyed the excursion's history. No test below `App` can observe that, because the drain and
    /// the clear never meet there.
    ///
    /// Returns the handle the escapes land in, or `None` if the renderer could not be built. Only the
    /// construction differs from the production path — everything after it runs
    /// [`Self::adopt_fullscreen_renderer`], the same code, so this seam cannot drift from what it
    /// stands in for.
    #[cfg(test)]
    pub(crate) fn enter_fullscreen_captured(&mut self) -> Option<crate::altscreen::Captured> {
        let backend = self.terminal.backend().rebuild();
        let theme = self.state.theme.clone();
        let (alt, captured) = AltScreen::for_test(backend, theme).ok()?;
        self.adopt_fullscreen_renderer(alt);
        Some(captured)
    }

    /// Everything [`Self::enter_fullscreen`] does after the renderer exists, shared with
    /// [`Self::enter_fullscreen_captured`] so the two cannot diverge.
    fn adopt_fullscreen_renderer(&mut self, mut alt: AltScreen<B>) {
        // §B-12, pi `:264-270` — immediately after entering, before the first frame: latch the
        // terminal's protocol and suppress the one (iterm2) whose placements the alternate screen
        // cannot own. Undone by [`Self::stop_fullscreen`].
        alt.adopt_images(&mut self.state.transcript);
        // §B-1. The retained document is the ONLY thing the alternate screen has to paint, and it
        // grows exclusively inside `TranscriptView::drain_committed` (`transcript/view.rs:110-116`)
        // — so retention has to be on before the first drain of the excursion or the screen would
        // be permanently empty.
        //
        // [CYRUP-DELTA] against that field's own "set once, for the session's life" rule
        // (`transcript/view.rs:151-160`): the rule exists because turning retention off and back on
        // would splice two non-adjacent runs of history together with no gap marker and no
        // `retained_dropped` movement. [`Self::stop_fullscreen`] closes exactly that hole by
        // pairing the `false` with `clear_document`, which empties the document AND advances
        // `retained_dropped` by what it dropped (`transcript/view.rs:188-194`) — so the next
        // excursion starts from an empty document at a moved counter, which is a gap the scroll
        // model already understands, not a silent splice. The composition root remains free to set
        // the flag itself at boot, and this is idempotent with that.
        self.state.transcript.set_retain_document(true);
        self.altscreen = Some(alt);
    }

    /// Take the alternate screen down and give the inline renderer its terminal back, answering
    /// whether one was live — pi's `previousUi.stop()` (`interactive-mode.ts:857`).
    ///
    /// Reached from two places: [`Self::install_renderer`]'s `Regular` arm, and `App::run`'s exit
    /// path, which must run it before `drain_and_restore` — otherwise the alternate screen would be
    /// left standing until `App` itself is dropped, i.e. *after* the terminal has been restored.
    ///
    /// The order is upstream's teardown order and each step undoes exactly one of
    /// [`Self::enter_fullscreen`]'s: stop the renderer (`:306-308` deletes placements, `:306`
    /// disables mouse reporting, `:315` leaves the screen), then put the image capabilities and the
    /// transcript's graphics gate back (`:330-333`), then retire the retained document.
    /// `preserve_screen` is the caller's: `false` repaints the excursion's document into the main
    /// screen's scrollback on the way out, `true` skips it. Both of cyrup's teardowns pass `false` —
    /// see [`crate::AltScreen::stop`] for why neither can rely on the inline renderer to re-render
    /// that history the way upstream's does.
    pub(crate) fn stop_fullscreen(&mut self, preserve_screen: bool) -> bool {
        let Some(mut alt) = self.altscreen.take() else {
            return false;
        };
        alt.stop(preserve_screen);
        alt.restore_images(&mut self.state.transcript, &self.state.image_renderer);
        drop(alt);
        // The pair that keeps the §B-1 flag's "set once" rule honest across a second excursion —
        // see [`Self::enter_fullscreen`]. `clear_document` first: it is what advances
        // `retained_dropped`, and turning retention off does not by itself empty anything.
        self.state.transcript.clear_document();
        self.state.transcript.set_retain_document(false);
        true
    }
}
