//! The pure core of every terminal reader: raw input bytes and the clock in, crossterm events out.
//!
//! [`ByteDecoder`] frames bytes with [`StdinBuffer`] and decodes each frame with [`decode`]; on the
//! way it takes the terminal's own replies out of the stream — a mode-`2031` colour-scheme report
//! ([`crate::color_scheme::offer_report`]) and a colour or DA1 reply a query is waiting for
//! ([`crate::terminal_query::offer_reply`]) — so they never become typing.
//!
//! It is platform-neutral. The unix fd loop (`super::reader`) feeds it `read(2)` chunks; on Windows
//! the console reader (`super::console`) feeds it the bytes a virtual-terminal-input console
//! delivers (TUI-163). One decoder, so the two platforms cannot disagree about what a byte means.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::Event;

use super::decode::{Decoded, decode};
use super::frame::{Frame, StdinBuffer};

/// The pure half of a reader: bytes and the clock in, events out. The unix `TtyReader` and the
/// Windows `ConsoleReader` drive it; tests drive it directly with injected time.
#[derive(Debug)]
pub(crate) struct ByteDecoder {
    buffer: StdinBuffer,
    frames: Vec<Frame>,
}

impl ByteDecoder {
    /// A decoder holding a lone `ESC` for `escape_timeout`.
    pub(crate) fn new(escape_timeout: Duration) -> Self {
        Self {
            buffer: StdinBuffer::new(escape_timeout),
            frames: Vec::new(),
        }
    }

    /// When a held sequence falls due, if one is held.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.buffer.deadline()
    }

    /// One `read(2)` chunk, received at `now`.
    pub(crate) fn feed(&mut self, bytes: &[u8], now: Instant, out: &mut Vec<Event>) {
        self.buffer.push(bytes, now, &mut self.frames);
        self.decode_frames(out);
    }

    /// Flush the held sequence if its timeout has passed by `now`.
    pub(crate) fn flush_due(&mut self, now: Instant, out: &mut Vec<Event>) {
        if self.buffer.deadline().is_some_and(|due| due <= now) {
            self.buffer.flush(&mut self.frames);
            self.decode_frames(out);
        }
    }

    fn decode_frames(&mut self, out: &mut Vec<Event>) {
        // The last colour-scheme report of this read. Pi hands a back-to-back burst of them to the
        // listener as one report whose scheme is the final frame's (`parseTerminalColorScheme
        // Report`, `terminal-colors.ts:85-91`), so the listener hears it once.
        let mut reported_scheme = None;
        for frame in self.frames.drain(..) {
            // `CSI ? 997 ; N n`, the mode-2031 notification: consumed ahead of every input listener
            // and every query, exactly like a colour reply (`tui.ts:1048`), and never typing.
            if let Frame::Seq(bytes) = &frame
                && let Some(scheme) = crate::color_scheme::offer_report(bytes)
            {
                reported_scheme = Some(scheme);
                continue;
            }
            // A colour or DA1 reply that a query still waiting on the terminal owns — one that
            // arrived after the boot probe's deadline — completes that query here (pi's
            // `consumeTerminalColorResponse` runs ahead of every input listener, `tui.ts:1045`).
            // Nothing else is touched: anything the queries do not own is decoded as before.
            if let Frame::Seq(bytes) = &frame
                && crate::terminal_query::offer_reply(bytes)
            {
                continue;
            }
            if let Decoded::Event(ev) = decode(frame) {
                out.push(ev);
            }
        }
        if let Some(scheme) = reported_scheme {
            crate::color_scheme::deliver(scheme);
        }
    }
}
