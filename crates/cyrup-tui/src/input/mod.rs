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
//! - `reader` (unix only) — the fd loop: `poll(2)` + `read(2)` on the tty, `SIGWINCH` as
//!   [`Event::Resize`], and a pause while `$EDITOR` or a suspend owns the terminal.
//!
//! On Windows crossterm's console-event reader stays in place (`crossterm_input_stream`'s
//! `cfg(not(unix))` arm): the console does not deliver VT bytes unless asked to, and asking is out
//! of scope.
//!
//! [`Event`]: ratatui::crossterm::event::Event
//! [`Event::Resize`]: ratatui::crossterm::event::Event::Resize

pub(crate) mod decode;
pub(crate) mod frame;
#[cfg(unix)]
pub(crate) mod reader;
