//! cyrup's own terminal input layer: raw `read(2)` chunks in, crossterm [`Event`]s out.
//!
//! This is the port of pi's `packages/tui/src/stdin-buffer.ts` (@v0.87.1) plus the decoder pi's
//! `keys.ts` stands in for. It exists because crossterm's input layer cannot be given bytes:
//! `parse_event` is `pub(crate)`, the event source is private, and `event::poll` drains the tty into
//! crossterm's own parser. Every defect filed against the old event-level repairs
//! (`crate::escape_reassembly`, `crate::stray_reply`) came from that: a Kitty duplicate that looks
//! identical to typing once it is an event (`TUI-046`), a DCS/APC reply shredded into keystrokes
//! (`TUI-047`), and an 8-bit meta byte that produces no event at all (`TUI-050`).
//!
//! Three layers, each testable on its own:
//!
//! - [`frame`] — [`frame::StdinBuffer`], the framer. It splits raw bytes into complete escape
//!   sequences, characters, bracketed pastes and 8-bit meta bytes, holding a partial sequence
//!   until the next read completes it or its timeout expires. Time is injected.
//! - [`decode`] — one [`frame::Frame`] to one crossterm [`Event`], a swallowed terminal reply, or
//!   nothing. crossterm's `parse_event` rules, so every consumer keeps seeing the events it saw.
//! - [`bytes`] — [`bytes::ByteDecoder`], the framer and the decoder together, with the two
//!   interceptions every reader needs: the mode-2031 colour-scheme report
//!   ([`crate::color_scheme::offer_report`]) and the replies to cyrup's own terminal queries.
//! - `reader` (unix only) — the fd loop: `poll(2)` + `read(2)` on the tty, `SIGWINCH` as
//!   [`Event::Resize`], and a pause while `$EDITOR` or a suspend owns the terminal.
//!
//! On Windows (TUI-163) the console delivers the terminal's bytes only when asked to, with
//! `ENABLE_VIRTUAL_TERMINAL_INPUT`. `console_win` asks, reads the console's records, and
//! [`console`] turns them back into the bytes the terminal sent and feeds the same
//! [`bytes::ByteDecoder`] — so the colour-scheme report is received there too. A console that
//! refuses virtual-terminal input keeps crossterm's console-event reader
//! (`crossterm_input_stream`'s fallback), and the report is never asked for there.
//!
//! [`Event`]: ratatui::crossterm::event::Event
//! [`Event::Resize`]: ratatui::crossterm::event::Event::Resize

pub(crate) mod bytes;
pub(crate) mod console;
#[cfg(windows)]
pub(crate) mod console_win;
pub(crate) mod decode;
pub(crate) mod frame;
#[cfg(unix)]
pub(crate) mod reader;
