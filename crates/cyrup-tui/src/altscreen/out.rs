//! The renderer's escape-sequence sink — cyrup's counterpart to pi's injected `Terminal`.
//!
//! Upstream's `Terminal` is an **interface** (`packages/tui/src/terminal.ts:60`) and
//! `TuiAltScreen`'s constructor takes one (`tui-alt-screen.ts:205-211`), which is the whole reason
//! `packages/tui/test/tui-alt-screen.test.ts` can assert on written bytes: its `VirtualTerminal`
//! (`test/virtual-terminal.ts:11`) implements that interface and records every `write` (`:50`).
//!
//! This module restores that seam. Before it, seven sites in this module tree wrote escapes
//! straight to `io::stdout()` — `terminal.rs` (alt-screen enter/leave), `mouse.rs` (reporting
//! enable/disable), `images.rs` (kitty deletes) and `mod.rs` (the exit repaint). Those bytes never
//! pass through ratatui's `Backend`, so a [`ratatui::backend::TestBackend`] observes none of them
//! and roughly half of upstream's suite could not be expressed at all.
//!
//! Production is [`Out::Stdout`] and is byte-identical to the `io::stdout()` calls it replaces:
//! same `queue!` payloads, same order, same flush points.
//!
//! ## Why the guards own one rather than borrow one
//!
//! [`super::terminal::TerminalSetup`], [`super::mouse::MouseSetup`] and
//! [`super::images::ImageLifecycle`] each restore terminal state from `Drop`. `Drop::drop` takes
//! only `&mut self`, so a sink passed per-call could not reach the un-taken exit — the one path
//! where restoring matters most. Each guard therefore holds its own handle, and [`Out`] is `Clone`
//! so one renderer can hand the same capture buffer to all of them.

use std::io::{self, Write};
#[cfg(test)]
use std::sync::{Arc, Mutex};

/// Where this module tree's escape sequences go.
///
/// See the module doc. `Clone` is by-handle, not by-content: cloning [`Out::Stdout`] opens another
/// `io::stdout()` handle (they share one lock), and cloning [`Out::Capture`] shares the same
/// buffer, so every guard in one renderer appends to a single ordered transcript.
pub(super) enum Out {
    /// The real terminal.
    Stdout(io::Stdout),
    /// Test-only capture. `cfg`-gated so a release build carries neither the variant nor the match
    /// arm that reads it.
    #[cfg(test)]
    Capture(Arc<Mutex<Vec<u8>>>),
}

/// A handle onto captured bytes, shared with every guard the renderer built.
#[cfg(test)]
pub(crate) type Captured = Arc<Mutex<Vec<u8>>>;

impl Default for Out {
    fn default() -> Self {
        Self::Stdout(io::stdout())
    }
}

impl Clone for Out {
    fn clone(&self) -> Self {
        match self {
            Self::Stdout(_) => Self::Stdout(io::stdout()),
            #[cfg(test)]
            Self::Capture(buf) => Self::Capture(Arc::clone(buf)),
        }
    }
}

impl Out {
    /// A sink over a fresh capture buffer, plus the handle a test reads it back through.
    #[cfg(test)]
    pub(super) fn capture() -> (Self, Captured) {
        let buf: Captured = Arc::new(Mutex::new(Vec::new()));
        (Self::Capture(Arc::clone(&buf)), buf)
    }
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(out) => out.write(buf),
            // A poisoned lock means another thread panicked mid-write. Dropping the bytes is the
            // right answer for a test transcript — the assertion that reads it will fail and say
            // so, which beats propagating a second failure out of a `Drop`.
            #[cfg(test)]
            Self::Capture(cell) => {
                if let Ok(mut held) = cell.lock() {
                    held.extend_from_slice(buf);
                }
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(out) => out.flush(),
            #[cfg(test)]
            Self::Capture(_) => Ok(()),
        }
    }
}

/// Read a capture buffer back as a string, for assertions.
///
/// Lossy on purpose: the transcript is escape sequences interleaved with rendered text, and a test
/// that wants to find `\x1b[?1002h` in it should not have to care whether some unrelated row
/// carried invalid UTF-8.
#[cfg(test)]
pub(crate) fn captured_text(buf: &Captured) -> String {
    buf.lock().map_or_else(|_| String::new(), |held| String::from_utf8_lossy(&held).into_owned())
}
