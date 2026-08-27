//! The **exit repaint** — leaving the alternate screen without the session's transcript vanishing
//! with it. cyrup's port of the `preserveScreen == false` half of pi's `afterTerminalStop`
//! (`packages/tui/src/tui-alt-screen.ts:311-333` @v0.84.3, the row loop at `:322-327`).
//! ADR-0005 §Decision B-13.
//!
//! # Why an exit needs a repaint at all and a mode switch does not
//! The alternate screen is a second, non-scrolling grid the terminal hands back and then throws
//! away: nothing painted into it reaches the user's scrollback, so `\x1b[?1049l` on its own ends the
//! session with the whole conversation gone and the pre-launch shell prompt back as if nothing had
//! run. Upstream's answer is to write the last rendered document onto the *main* screen immediately
//! after leaving, one row at a time from wherever the restored cursor sits, so the terminal scrolls
//! it into history exactly as the inline renderer's `insert_before` does every frame
//! (R-ARCH-TUI-003; `app/draw.rs:174-179`).
//!
//! `preserveScreen` is upstream's name for the other exit — "leave renderer output in place for
//! another TUI taking over the same terminal" (`tui.ts:286-289`), which is ADR-0005 §B-14's live
//! mode switch. There the incoming renderer is about to paint the same conversation inline, so a
//! repaint would put the document on screen twice; [`repaint`] is therefore total on the flag and
//! writes nothing when it is set, rather than leaving that decision to each call site.
//!
//! # cyrup passes the document in; upstream re-renders it
//! Upstream calls `this.render(width)` at exit (`:318`) and parks the result in a `lastDocument`
//! field it writes at `:319` and reads at `:323-325` — a field whose only other use is being
//! emptied on the next entry (`:272`), so it is a local in all but declaration. cyrup cannot re-render
//! here: the rows are a projection of [`crate::AppState`]'s transcript and theme, which no renderer
//! in this tree may hold (`altscreen/mod.rs`, structural rule 2), and the teardown path has no
//! `AppState`. It does not need to. ADR-0005 §B-5's per-frame rendered-document cache already holds
//! exactly what `render(width)` would return, so the rows arrive here as an argument and a second
//! copy — which would go stale the frame after it was taken — never exists.
//!
//! Note that this is the **document**, not the composited screen: upstream repaints `render(width)`,
//! which is the layout root (`:245-247`), and deliberately not the frame the user was just looking
//! at. The editor, the status band and any live flash are chrome for a viewport that is going away;
//! only the transcript belongs in scrollback.
//!
//! # Two of upstream's four per-row normalisations do not port
//! `:318-321` runs each row through four steps before writing it. Two are inapplicable here and are
//! not omissions:
//!
//! - **The OSC 133 zone-prefix strip** (`:318`, against the pattern at `:62`) has nothing to strip:
//!   cyrup emits no shell-integration marks anywhere — which is also why ADR-0005 §B-10 walks
//!   [`crate::Entry::User`] for the prompt jump instead of the marks upstream walks.
//! - **The cursor-marker strip and `applyLineResets`** (`:319`, `tui.ts:1160-1168`) both exist
//!   because upstream's rows are assembled ANSI strings that can carry an unterminated SGR run into
//!   the next row. cyrup's rows are [`Line`]s of styled [`ratatui::text::Span`]s, and
//!   [`write_row`] emits each span through crossterm's `PrintStyledContent`, which restores what it
//!   set — so every row is already self-terminating.
//!
//! The other two do port and are applied by [`repaint`]: the per-row clip to the terminal width
//! (`:320`) and the `\r` + erase-line before each row (`:325`).
//!
//! # Where this is called from
//! Between `LeaveAlternateScreen` and the closing reset of
//! `super::terminal::TerminalSetup::leave` — the point that function's `preserve_screen == false`
//! branch marks, where autowrap has just been disabled for exactly this write (`:322`) and the
//! `\x1b[0m` + autowrap-back + `\r\n` that close the repaint (`:327`) have not been emitted yet.
//! This module writes **only** the rows: none of that framing, no synchronized-output bracket, and
//! no cursor or alternate-screen escape. Everything it does is inside the `?2026h`/`?2026l` bracket
//! its caller opened, which is what keeps the restore one visible transition rather than three.

use std::io::Write;

use ratatui::backend::IntoCrossterm;
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{ContentStyle, Print, PrintStyledContent};
use ratatui::crossterm::terminal::{Clear, ClearType};
use ratatui::text::Line;

use crate::text_width::truncate_line_to_width;

/// Repaint `document` onto the main screen — pi's `afterTerminalStop` row loop
/// (`tui-alt-screen.ts:322-327`), minus the framing its caller owns (see the module doc).
///
/// `width` is the terminal's current column count, floored at 1 exactly as upstream floors it
/// (`:317`); a row wider than that is hard-clipped rather than allowed to wrap, because autowrap is
/// off for this write and a wrapped row would silently double-space the history the user is about to
/// scroll through.
///
/// `preserve_screen` is pi's `TuiStopOptions.preserveScreen` (`tui.ts:286-289`): `true` selects the
/// `:315` branch, which repaints nothing because ADR-0005 §B-14's incoming renderer is about to
/// paint the same conversation. An empty `document` is the same no-op by a different route, and is
/// upstream's own output for a document with no rows.
///
/// Returns nothing and swallows every write error, matching
/// `super::terminal::TerminalSetup::leave` and the `startup_selector.rs:44-51` restore idiom this
/// sits inside: a terminal that rejects one row must not abandon the rest and strand the user
/// looking at a blank screen.
pub(super) fn repaint<W: Write>(
    out: &mut W,
    document: &[Line<'static>],
    width: u16,
    preserve_screen: bool,
) {
    if preserve_screen {
        return;
    }
    // `Math.max(1, this.terminal.columns)` (`:317`).
    let max = usize::from(width.max(1));
    for (row, line) in document.iter().enumerate() {
        // `if (row > 0) buffer += "\r\n"` (`:324`) — the separator goes BETWEEN rows, never after
        // the last one. The trailing newline that ends the repaint is the caller's (`:327`), so
        // emitting one here too would leave a blank line under every fullscreen session.
        if row > 0 {
            let _ = queue!(out, Print("\r\n"));
        }
        // `\r\x1b[2K` (`:325`). The main screen the terminal just restored still holds whatever was
        // under it, and the rows land on top of it: without the erase, a repainted row shorter than
        // the shell line beneath would leave that line's tail showing past its end.
        let _ = queue!(out, Print("\r"), Clear(ClearType::CurrentLine));
        // `visibleWidth(line) <= width ? line : sliceByColumn(line, 0, width, true)` (`:320`).
        // `truncate_line_to_width` returns the row untouched when it already fits, so the clone is
        // paid only by a row that a resize between the last frame and this exit left too wide; the
        // empty ellipsis is what makes it a hard cut rather than an elision.
        if line.width() <= max {
            write_row(out, line);
        } else {
            write_row(out, &truncate_line_to_width(line.clone(), max, ""));
        }
    }
}

/// Write one row's styled spans — the `${this.lastDocument[row]}` of `:325`, which upstream can
/// interpolate directly because its rows are already ANSI strings and cyrup's are not.
///
/// Each span is emitted through crossterm's `PrintStyledContent`, which sets only the colours and
/// attributes the span actually carries and restores them afterwards (crossterm `style.rs:424-462`).
/// That is what stands in for upstream's `applyLineResets` (`tui.ts:1160-1168`): a row cannot leak
/// its styling into the row below it, or into the shell prompt that follows the last one.
///
/// The row's own [`Line::style`] is patched under each span's, which is ratatui's own composition
/// order for a styled line — a line-level colour is the base and a span's overrides it, not the
/// other way round.
fn write_row<W: Write>(out: &mut W, line: &Line<'static>) {
    for span in &line.spans {
        let style: ContentStyle = line.style.patch(span.style).into_crossterm();
        let _ = queue!(out, PrintStyledContent(style.apply(&*span.content)));
    }
}
