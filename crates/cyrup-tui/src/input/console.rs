//! TUI-163 — the Windows console's input, read as the bytes the terminal sent.
//!
//! With `ENABLE_VIRTUAL_TERMINAL_INPUT` on, the console stops translating the terminal's escape
//! sequences into key records: every byte the terminal sends arrives as the character of a key-down
//! record (an arrow is `ESC`, `[`, `A`; a mode-`2031` colour-scheme report is `ESC [ ? 9 9 7 ; 2
//! n`). crossterm's console reader maps records by virtual-key code, so it would read that stream
//! as typing — which is why cyrup never asked for the reports on Windows. This module turns the
//! records back into bytes and hands them to the same [`ByteDecoder`] the unix reader uses, so the
//! report reaches [`crate::color_scheme::offer_report`] and never the editor, and every key decodes
//! by the one set of rules (`super::decode`).
//!
//! The OS half — the console mode, the wait and `ReadConsoleInputW` — is `super::console_win`.
//! This half is pure (records and the clock in, events out) and platform-neutral, so its rules are
//! tested on every host.
// Off Windows only the tests drive it.
#![cfg_attr(not(windows), allow(dead_code))]

use std::io;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::Event;

use super::bytes::ByteDecoder;

/// One console input record, as far as the reader needs it (`INPUT_RECORD`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsoleRecord {
    /// `KEY_EVENT`: `key_down` is `bKeyDown`, `unit` the UTF-16 code unit in `uChar.UnicodeChar`
    /// (`0` for a key that produced no character), `repeat` is `wRepeatCount`.
    Key {
        key_down: bool,
        unit: u16,
        repeat: u16,
    },
    /// `WINDOW_BUFFER_SIZE_EVENT`.
    Resize,
    /// `MOUSE_EVENT`, `FOCUS_EVENT`, `MENU_EVENT`. With virtual-terminal input on, the terminal
    /// reports mouse and focus as escape sequences (cyrup enables them with VT escapes,
    /// `crate::altscreen::mouse`), which arrive as key records like everything else; a record of
    /// these kinds carries nothing the bytes do not.
    Other,
}

/// Records in, events out: the character of every key-down record, as UTF-8, through a
/// [`ByteDecoder`].
#[derive(Debug)]
pub(crate) struct ConsolePump {
    bytes: ByteDecoder,
    /// A high surrogate whose low half has not arrived yet (a character outside the BMP comes as
    /// two records).
    high: Option<u16>,
    /// The bytes of the records pushed so far in this batch.
    chunk: Vec<u8>,
}

impl ConsolePump {
    /// A pump holding a lone `ESC` for `escape_timeout`.
    pub(crate) fn new(escape_timeout: Duration) -> Self {
        Self {
            bytes: ByteDecoder::new(escape_timeout),
            high: None,
            chunk: Vec::new(),
        }
    }

    /// When a held sequence falls due, if one is held.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.bytes.deadline()
    }

    /// One batch of records, read at `now`, in order. A resize is reported where it fell between
    /// the keys, with the size `size` reads, as the unix reader reports `SIGWINCH`.
    pub(crate) fn push(
        &mut self,
        records: &[ConsoleRecord],
        now: Instant,
        size: &dyn Fn() -> io::Result<(u16, u16)>,
        out: &mut Vec<Event>,
    ) {
        for record in records {
            match *record {
                ConsoleRecord::Key {
                    key_down: true,
                    unit,
                    repeat,
                } if unit != 0 => {
                    for _ in 0..repeat.max(1) {
                        self.unit(unit);
                    }
                }
                ConsoleRecord::Resize => {
                    self.feed(now, out);
                    if let Ok((cols, rows)) = size() {
                        out.push(Event::Resize(cols, rows));
                    }
                }
                ConsoleRecord::Key { .. } | ConsoleRecord::Other => {}
            }
        }
        self.feed(now, out);
    }

    /// Flush the held sequence if its timeout has passed by `now`.
    pub(crate) fn flush_due(&mut self, now: Instant, out: &mut Vec<Event>) {
        self.bytes.flush_due(now, out);
    }

    fn feed(&mut self, now: Instant, out: &mut Vec<Event>) {
        if !self.chunk.is_empty() {
            self.bytes.feed(&self.chunk, now, out);
            self.chunk.clear();
        }
    }

    /// Append one UTF-16 code unit's worth of UTF-8. An unpaired surrogate becomes U+FFFD, as a
    /// lossy decode of the same units would.
    fn unit(&mut self, unit: u16) {
        let ch = match (self.high.take(), unit) {
            (None, 0xD800..=0xDBFF) => {
                self.high = Some(unit);
                return;
            }
            (Some(high), 0xDC00..=0xDFFF) => {
                char::decode_utf16([high, unit]).next().and_then(Result::ok)
            }
            (Some(_), _) => {
                // The high half had no partner: it is replaced, and this unit is taken on its own.
                self.push_char(char::REPLACEMENT_CHARACTER);
                return self.unit(unit);
            }
            (None, _) => char::decode_utf16([unit]).next().and_then(Result::ok),
        };
        self.push_char(ch.unwrap_or(char::REPLACEMENT_CHARACTER));
    }

    fn push_char(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        self.chunk
            .extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
