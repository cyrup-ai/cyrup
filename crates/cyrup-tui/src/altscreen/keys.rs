//! **Viewport keybindings** — the eight `tui.altScreen.*` ids bound to the scroll model, and the
//! *shadowing* rule that makes them win over the editor in fullscreen and lose to it in regular.
//! cyrup's port of the key tail of pi's `TuiAltScreen.handleViewportInput`
//! (`packages/tui/src/tui-alt-screen.ts:600-644` @v0.84.3) over the definitions at
//! `packages/tui/src/keybindings.ts:159-209`. ADR-0005 §Decision B-9 (the ADR's `:44-52` / `:153-179`
//! citations are the @v0.84.1 line numbering for the same two spans).
//!
//! The table itself is not here: it is [`crate::keymap::AltScreenKeymap`], beside every other
//! configurable map, because a `keybindings.json` entry has to reach it through the same
//! `merge_json` path the editor, selector, tree and session maps use. What lives here is the half
//! upstream keeps in the renderer — which scroll operation each action performs, and how far.
//!
//! # The shadowing rule is structural, and it has two halves
//! `keybindings.ts:159` — "These intentionally shadow the unmodified editor bindings in fullscreen
//! mode" — is behaviour, not a comment. Upstream implements it by *position*: the alternate screen
//! registers `handleViewportInput` as an **input listener** (`tui-alt-screen.ts:227`), and the
//! listener loop in `handleTerminalInput` runs to completion before the focused component is ever
//! offered the key (`tui.ts:834-848` against `tui.ts:892-897`). A listener that answers
//! `{ consume: true }` ends the dispatch; one that answers `undefined` lets the key fall through to
//! the editor unchanged.
//!
//! cyrup reproduces both halves, and needs both because the inline renderer must not change
//! (ADR-0005 §Decision B):
//!
//! 1. **Position.** [`route`] returns a [`KeyOutcome`], and [`KeyOutcome::Pass`] is upstream's
//!    `undefined`: the alternate screen's dispatcher (§B-3) offers the event onward to
//!    [`crate::App::handle_input`], which is where the overlay stack, the selector stack, the
//!    global [`crate::Keymap`] and the [`crate::EditorKeymap`] still live. There is exactly one
//!    editor in either mode, so anything this file does not claim keeps working in fullscreen.
//! 2. **Mode.** [`crate::keymap::AltScreenKeymap::action_in_mode`] resolves nothing at all under
//!    [`TuiRenderMode::Regular`], so even a caller that routed an inline key through here would
//!    move no viewport. That is why `pageUp` in an inline session still reaches
//!    `tui.editor.pageUp` and cyrup's `app.pageUp` exactly as it did before this ADR — the two
//!    tables are disjoint at resolution time, not merely at call-site discipline.
//!
//! # A release consumes but does not act
//! Every one of upstream's arms is `if (!isRelease) …; return { consume: true }` (`:601-644`, over
//! `isKeyRelease`, `keys.ts:527`): the *release* of a bound viewport chord is swallowed rather than
//! forwarded, so it cannot arrive at the editor as a second event. [`route`] answers
//! [`KeyOutcome::Handled`] for it without moving the view.
//!
//! # Two gates the caller owns, not this module
//! Upstream checks `shouldDeferViewportInputToOverlay()` (`:538-540`) immediately before the page
//! arms (`:599`), so a focused overlay keeps its own `pageUp`; and it tests the four
//! `tui.altScreen.search*` ids ahead of them (`:582-597`). Both are dispatcher-shaped — the first
//! needs the overlay stack, which is [`crate::AppState`]'s and not the renderer's (`altscreen/mod.rs`,
//! rule 2) — so both sit *around* this call, in ADR-0005 §B-3, exactly as [`super::wheel::route`]'s
//! own deferral does.
//!
//! # The ids upstream has that cyrup does not register
//! `tui.altScreen.lineUp` / `lineDown` (`keybindings.ts:176-183`) and the four
//! `tui.altScreen.search*` ids (`:192-207`) are deliberately absent: ADR-0005 §Decision C
//! enumerates **eight**, and transcript search is not among §Decision B's units at all — see
//! [`super::scroll`]'s module doc, where the same omission leaves `activeSearch` permanently
//! unset. Registering an id whose handler cannot exist would put it in the user's keybindings
//! surface as a binding that silently does nothing.

use ratatui::crossterm::event::{KeyEvent, KeyEventKind};

use super::scroll::{self, ScrollState};
use super::TuiRenderMode;
use crate::keymap::{AltScreenAction, AltScreenKeymap};

/// Rows a page scroll leaves behind for continuity — pi's `PAGE_SCROLL_OVERLAP`
/// (`tui-alt-screen.ts:64`; ADR-0005 §Decision C cites `:57`, the @v0.84.1 line), applied at
/// `:603` and `:609`.
///
/// A `usize` because the viewport height it is subtracted from is one
/// ([`scroll::viewport_height`]); upstream's `number` is the same quantity.
pub(super) const PAGE_SCROLL_OVERLAP: usize = 4;

/// What [`route`] did with one key event — cyrup's form of pi's `TuiInputListenerResult`, the
/// `{ consume?: boolean; data?: string } | undefined` an input listener answers with
/// (`tui.ts:49-50`), read by the listener loop at `tui.ts:837-843`. The `data` half — a listener
/// REWRITING the event for the listeners behind it (`tui.ts:841-843`) — has no counterpart here:
/// `handleViewportInput` never returns one (`tui-alt-screen.ts:542-644`).
///
/// The two prompt variants exist because the jump itself is ADR-0005 §B-10's `prompt_nav`, which
/// needs the rendered document this module never sees: upstream's `scrollToPrompt(-1)` / `(1)`
/// (`tui-alt-screen.ts:630`, `:634`) scan rendered rows for an OSC 133 mark, and cyrup walks the
/// retained `Entry` list instead. Reporting the direction keeps the binding resolved here — where
/// the shadowing rule is — while the walk stays where the document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    /// No `tui.altScreen.*` binding matched: upstream's `return undefined`. The caller MUST offer
    /// the event onward to [`crate::App::handle_input`] — that fall-through is what keeps the
    /// editor, the selector stack and the overlays alive in fullscreen.
    Pass,
    /// The event was a viewport binding and is fully dealt with: upstream's `{ consume: true }`.
    /// Also the answer for the *release* of a bound chord, which consumes without moving.
    Handled,
    /// `tui.altScreen.previousPrompt` — the caller runs ADR-0005 §B-10's backward prompt walk
    /// (`tui-alt-screen.ts:629-632`). Consumed either way, including when no earlier prompt exists.
    PreviousPrompt,
    /// `tui.altScreen.nextPrompt` — the forward walk (`tui-alt-screen.ts:633-636`).
    NextPrompt,
}

/// Rows one page moves — pi's `Math.max(1, viewportHeight - PAGE_SCROLL_OVERLAP)` (`:603`, `:609`).
///
/// The floor is upstream's and is load-bearing: a viewport shorter than [`PAGE_SCROLL_OVERLAP`]
/// would otherwise ask for a *negative* page and page the wrong way. A 30-row viewport moves 26
/// rows; a 3-row viewport moves 1, because the subtraction saturates at zero before the floor
/// applies. The result is therefore always `>= 1`, which is what makes [`i32::saturating_neg`]
/// below exact rather than defensive.
fn page_lines(scroll: &ScrollState) -> i32 {
    let rows = scroll::viewport_height(scroll)
        .saturating_sub(PAGE_SCROLL_OVERLAP)
        .max(1);
    // A viewport taller than `i32::MAX` rows cannot exist; the fallback is unreachable and only
    // spells the conversion without the `unwrap` the workspace lints deny.
    i32::try_from(rows).unwrap_or(i32::MAX)
}

/// Rows one half page moves — pi's `Math.max(1, Math.floor(viewportHeight / 2))` (`:614`, `:618`).
///
/// Rust's integer division on `usize` is already the floor, so the port is the `max` alone.
fn half_page_lines(scroll: &ScrollState) -> i32 {
    let rows = (scroll::viewport_height(scroll) / 2).max(1);
    i32::try_from(rows).unwrap_or(i32::MAX)
}

/// Offer one key event to the viewport bindings — the key tail of pi's `handleViewportInput`
/// (`tui-alt-screen.ts:600-644`), minus the search arms and the overlay gate the caller owns (see
/// the module doc).
///
/// `mode` is what makes this safe to call from a dispatcher that does not know which renderer is
/// live: under [`TuiRenderMode::Regular`] it resolves nothing and every event answers
/// [`KeyOutcome::Pass`], so the inline renderer's routing is bit-for-bit what it was
/// (ADR-0005 §Decision B — fullscreen is an additional mode, not a replacement).
///
/// The heights the two page sizes are derived from are [`ScrollState`]'s, set by
/// [`scroll::update_layout`] on the frame just drawn — upstream reads the same live
/// `getPrimaryScrollView().viewportHeight` (`:603`). All four scroll operations clamp internally
/// and mark scrollbar activity only when they actually moved, so a `pageUp` at the top of the
/// document is a consumed no-op here exactly as it is upstream (`components/scroll-view.ts:140-154`).
pub(super) fn route(
    scroll: &mut ScrollState,
    ev: &KeyEvent,
    keys: &AltScreenKeymap,
    mode: TuiRenderMode,
) -> KeyOutcome {
    let Some(action) = keys.action_in_mode(ev, mode) else {
        return KeyOutcome::Pass;
    };
    // `if (!isRelease) …; return { consume: true }` (`:601-644`): the release of a bound chord is
    // swallowed, never acted on and never forwarded.
    if matches!(ev.kind, KeyEventKind::Release) {
        return KeyOutcome::Handled;
    }
    // Both page sizes read the viewport height, so they are measured BEFORE the mutable borrow the
    // scroll call takes — the order is a borrow requirement, not a preference.
    match action {
        AltScreenAction::PageUp => {
            let lines = page_lines(scroll);
            scroll::scroll_by(scroll, lines.saturating_neg());
        }
        AltScreenAction::PageDown => {
            let lines = page_lines(scroll);
            scroll::scroll_by(scroll, lines);
        }
        AltScreenAction::HalfPageUp => {
            let lines = half_page_lines(scroll);
            scroll::scroll_by(scroll, lines.saturating_neg());
        }
        AltScreenAction::HalfPageDown => {
            let lines = half_page_lines(scroll);
            scroll::scroll_by(scroll, lines);
        }
        AltScreenAction::Top => scroll::scroll_to_top(scroll),
        AltScreenAction::Bottom => scroll::scroll_to_bottom(scroll),
        // The walk itself is ADR-0005 §B-10's; see [`KeyOutcome::PreviousPrompt`].
        AltScreenAction::PreviousPrompt => return KeyOutcome::PreviousPrompt,
        AltScreenAction::NextPrompt => return KeyOutcome::NextPrompt,
    }
    KeyOutcome::Handled
}
