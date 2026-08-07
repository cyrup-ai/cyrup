//! Kitty keyboard-protocol **capability negotiation** — the port of Pi's
//! `queryAndEnableKittyProtocol` / `parseKeyboardProtocolNegotiationSequence` /
//! `handleKeyboardProtocolNegotiationSequence` (`pi/packages/tui/src/terminal.ts:15-37`,
//! `:213-251`).
//!
//! # What Pi does
//!
//! On start Pi writes one composite sequence (`terminal.ts:17`):
//!
//! ```text
//! ESC [ > 7 u      push the desired flags (1 disambiguate | 2 event types | 4 alternate keys)
//! ESC [ ? u        ask which flags are now in effect
//! ESC [ c          Primary Device Attributes — the sentinel every VT-class terminal answers
//! ```
//!
//! The push has to come FIRST: `CSI ? u` reports the *top of the terminal's flag stack*, so a query
//! issued before the push would report `0` even on a Kitty-capable terminal. The DA1 sentinel is what
//! removes the need for a startup timeout — replies come back in order, so a DA1 that arrives with no
//! `CSI ? <flags> u` in front of it proves the terminal has no Kitty support to report.
//!
//! Pi then decides (`:228-250`): non-zero flags ⇒ the Kitty protocol is live; `0` flags, or DA1 with
//! no flags report, ⇒ it is not, and Pi falls back to xterm's `modifyOtherKeys` (`CSI > 4 ; 2 m`,
//! `:320-324`, undone with `CSI > 4 ; 0 m` at `:326-330`).
//!
//! # What cyrup does, and the one deliberate delta
//!
//! Everything above except the `modifyOtherKeys` write. [`negotiate`] pushes (the caller's
//! `PushKeyboardEnhancementFlags`, [`crate::App::into_stdout`]), queries, consumes the reply behind
//! the same DA1 sentinel [`crate::terminal_query`] uses, and records the outcome in a process-global
//! ([`current`]) so the rest of the TUI — and the user-facing diagnostics — can tell whether modified
//! keys are actually disambiguated.
//!
//! `[CYRUP-DELTA]` **`modifyOtherKeys` is negotiated but not enabled.** Pi hand-rolls its key parser
//! (`tui/src/keys.ts`) and therefore understands xterm's `CSI 27 ; <mod> ; <code> ~` reports.
//! cyrup delegates key decoding to crossterm, whose `parse_csi_special_key_code`
//! (`crossterm-0.29.0/src/event/sys/unix/parse.rs:619-657`) accepts only the standard `1..=34`
//! parameter set and returns `could_not_parse_event_error()` for `27` — xterm's DEFAULT
//! `formatOtherKeys=0` shape, which no escape sequence can switch to CSI-u. Writing `CSI > 4 ; 2 m`
//! would therefore make every modified key on an xterm-family terminal unparseable, breaking
//! `Ctrl+<letter>` on exactly the terminals the fallback exists to help. The write stays out until
//! cyrup's input path can decode that form; the negotiation that would drive it is here and tested,
//! and [`KeyboardProtocol::Legacy`] is the state it would key off.
//!
//! The teardown asymmetry is unchanged and matches Pi: the pushed flags are popped unconditionally
//! (`PopKeyboardEnhancementFlags` in [`crate::panic_hook::restore_terminal_best_effort`]) — Pi's
//! `stop()` pops on `keyboardProtocolPushed || _kittyProtocolActive`, and cyrup always pushes, so the
//! condition is always true.
//!
//! # Why this may only run at startup
//!
//! [`negotiate`] reads stdin directly, so it is safe only in the window
//! [`crate::terminal_query`]'s module docs describe: after raw mode is on and BEFORE the crossterm
//! reader thread exists. `App::suspend` and the external-editor round trip re-push the flags while
//! that reader thread is live, so they must NOT re-query — they re-apply the decision this module
//! already recorded. A late `CSI ? <flags> u` is harmless in any case: crossterm's `?`-parameter CSI
//! arm terminates on `u` and yields an internal keyboard-enhancement event, not a keystroke.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

/// `CSI ? u` — Pi's flags query, the middle third of `KITTY_KEYBOARD_PROTOCOL_QUERY`
/// (`terminal.ts:17`). The leading `CSI > <flags> u` push is the caller's
/// (`PushKeyboardEnhancementFlags`) and the trailing `CSI c` sentinel is appended by
/// [`crate::terminal_query`]'s exchange, so this constant is only the query itself.
pub const KITTY_FLAGS_QUERY: &str = "\x1b[?u";

/// `CSI > 4 ; 2 m` — Pi's `enableModifyOtherKeys` (`terminal.ts:322`). Not written by cyrup; see the
/// `[CYRUP-DELTA]` in the module docs.
pub const MODIFY_OTHER_KEYS_ENABLE: &str = "\x1b[>4;2m";

/// `CSI > 4 ; 0 m` — Pi's `disableModifyOtherKeys` (`terminal.ts:328`). Not written by cyrup; see the
/// `[CYRUP-DELTA]` in the module docs.
pub const MODIFY_OTHER_KEYS_DISABLE: &str = "\x1b[>4;0m";

/// How long [`negotiate`] waits for the reply. `[CYRUP-DELTA]`: Pi needs no timeout because its
/// negotiation is a listener on an async event loop; cyrup's probe is synchronous, so it is bounded
/// exactly like [`crate::terminal_query`]'s (Pi's own 100 ms probe budget, `tui.ts:1174-1220`). A
/// terminal that answers nothing costs this once, at startup, and consumes no input.
pub const NEGOTIATION_TIMEOUT: Duration = Duration::from_millis(100);

/// What the terminal answered the flags query with — Pi's `_kittyProtocolActive` /
/// `_modifyOtherKeysActive` pair, collapsed into the one state they actually encode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyboardProtocol {
    /// A non-zero `CSI ? <flags> u` report: the Kitty protocol is live and modified keys arrive
    /// disambiguated (Pi `:233-240`).
    Kitty,
    /// The terminal answered — `0` flags, or the DA1 sentinel with no flags report — so it has no
    /// Kitty support. Pi enables `modifyOtherKeys` here (`:241`/`:246-248`); cyrup does not (module
    /// docs).
    Legacy,
    /// The terminal answered nothing at all (not a tty, cooked mode, or a terminal that answers
    /// neither the query nor DA1). Nothing is known and nothing is assumed.
    Unknown,
}

/// One recognized negotiation reply — Pi's `KeyboardProtocolNegotiationSequence`
/// (`terminal.ts:19-21`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NegotiationSequence {
    /// `CSI ? <flags> u`, Pi's `{ type: "kitty-flags", flags }`.
    KittyFlags(u32),
    /// `CSI ? <params> c`, Pi's `{ type: "device-attributes" }` sentinel.
    DeviceAttributes,
}

/// Pi `parseKeyboardProtocolNegotiationSequence` (`terminal.ts:23-34`) over one exact frame:
/// `/^\x1b\[\?(\d+)u$/` ⇒ Kitty flags, `/^\x1b\[\?[\d;]*c$/` ⇒ the DA1 sentinel, anything else ⇒
/// `None`.
pub fn parse_negotiation_sequence(sequence: &str) -> Option<NegotiationSequence> {
    let body = sequence.strip_prefix("\x1b[?")?;
    if let Some(flags) = body.strip_suffix('u') {
        // `\d+` — at least one digit, digits only. A value wider than `u32` is not a shape any
        // terminal produces; treat it as unparseable rather than saturating to a wrong answer.
        if flags.is_empty() || !flags.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return flags.parse::<u32>().ok().map(NegotiationSequence::KittyFlags);
    }
    if let Some(params) = body.strip_suffix('c') {
        // `[\d;]*` — Pi accepts an empty parameter list here.
        if params.bytes().all(|b| b.is_ascii_digit() || b == b';') {
            return Some(NegotiationSequence::DeviceAttributes);
        }
    }
    None
}

/// Pi `isKeyboardProtocolNegotiationSequencePrefix` (`terminal.ts:36-37`): the two shapes that are a
/// still-arriving negotiation reply rather than input. Pi buffers these for
/// `KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS` (`:16`, 150 ms) because its stdin arrives split
/// into per-sequence events; cyrup's [`negotiate`] reads raw bytes until the sentinel instead, so
/// this is used by the scanners below rather than by a buffering state machine.
pub fn is_negotiation_prefix(sequence: &str) -> bool {
    sequence == "\x1b["
        || (sequence.starts_with("\x1b[?")
            && sequence.bytes().skip(3).all(|b| b.is_ascii_digit() || b == b';'))
}

/// Locate a `CSI ? <flags> u` report anywhere inside a raw read (which also carries the DA1 sentinel
/// reply, and may carry a keystroke the user typed during startup) and return its flags.
///
/// Pi anchors its regex because its `StdinBuffer` has already split the stream per sequence; here the
/// bytes arrive unsplit, so we scan — the same shape as
/// [`crate::terminal_query::find_osc11_background_color`].
pub fn find_kitty_flags(buffer: &str) -> Option<u32> {
    let mut rest = buffer;
    while let Some(start) = rest.find("\x1b[?") {
        let tail = rest.get(start..)?;
        // The frame ends at its CSI final byte; only `u` is a flags report.
        if let Some(end) = tail.bytes().skip(3).position(|b| (0x40..=0x7e).contains(&b))
            && let Some(frame) = tail.get(..end + 4)
            && let Some(NegotiationSequence::KittyFlags(flags)) = parse_negotiation_sequence(frame)
        {
            return Some(flags);
        }
        rest = tail.get(3..)?;
    }
    None
}

/// Pi `handleKeyboardProtocolNegotiationSequence` (`terminal.ts:228-250`) as a pure decision over the
/// whole reply: non-zero flags ⇒ [`KeyboardProtocol::Kitty`]; `0` flags, or the DA1 sentinel with no
/// flags report in front of it, ⇒ [`KeyboardProtocol::Legacy`] (Pi's `enableModifyOtherKeys` branch);
/// nothing recognizable ⇒ [`KeyboardProtocol::Unknown`].
pub fn decide(reply: &str) -> KeyboardProtocol {
    match find_kitty_flags(reply) {
        Some(0) => KeyboardProtocol::Legacy,
        Some(_) => KeyboardProtocol::Kitty,
        None if crate::terminal_query::saw_device_attributes(reply.as_bytes()) => {
            KeyboardProtocol::Legacy
        }
        None => KeyboardProtocol::Unknown,
    }
}

/// The negotiated state, as a process-global — Pi keeps `_kittyProtocolActive` on its single
/// `ProcessTerminal` instance and mirrors it into the key parser (`setKittyProtocolActive`,
/// `terminal.ts:238`). There is exactly one terminal per process, and the readers ([`crate::App`]'s
/// re-entry paths, the startup diagnostics) are not all reachable from that one `App` value, so the
/// state lives here rather than on it.
static NEGOTIATED: AtomicU8 = AtomicU8::new(UNKNOWN);

const UNKNOWN: u8 = 0;
const KITTY: u8 = 1;
const LEGACY: u8 = 2;

/// The last negotiated protocol, or [`KeyboardProtocol::Unknown`] before [`negotiate`] has run.
pub fn current() -> KeyboardProtocol {
    match NEGOTIATED.load(Ordering::Relaxed) {
        KITTY => KeyboardProtocol::Kitty,
        LEGACY => KeyboardProtocol::Legacy,
        _ => KeyboardProtocol::Unknown,
    }
}

/// Record a negotiation outcome. Exposed for the startup wiring and for tests, which have no
/// terminal to negotiate with.
pub fn set_current(protocol: KeyboardProtocol) {
    NEGOTIATED.store(
        match protocol {
            KeyboardProtocol::Kitty => KITTY,
            KeyboardProtocol::Legacy => LEGACY,
            KeyboardProtocol::Unknown => UNKNOWN,
        },
        Ordering::Relaxed,
    );
}

/// Ask the terminal which keyboard-protocol flags are in effect and record the answer — Pi
/// `queryAndEnableKittyProtocol` (`terminal.ts:213-226`) plus its reply handler.
///
/// **Call this only from [`crate::App::into_stdout`]**, after the flags push and before the crossterm
/// reader thread exists; see the module docs. Returns the decision and stores it in [`current`].
/// Costs at most [`NEGOTIATION_TIMEOUT`] and consumes no byte the terminal did not send in reply.
pub fn negotiate() -> KeyboardProtocol {
    let reply = crate::terminal_query::exchange(KITTY_FLAGS_QUERY, NEGOTIATION_TIMEOUT);
    let decision = decide(reply.as_deref().unwrap_or_default());
    set_current(decision);
    decision
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    #[test]
    fn parses_pis_two_recognized_frames() {
        assert_eq!(parse_negotiation_sequence("\x1b[?1u"), Some(NegotiationSequence::KittyFlags(1)));
        assert_eq!(parse_negotiation_sequence("\x1b[?0u"), Some(NegotiationSequence::KittyFlags(0)));
        assert_eq!(
            parse_negotiation_sequence("\x1b[?29u"),
            Some(NegotiationSequence::KittyFlags(29))
        );
        assert_eq!(
            parse_negotiation_sequence("\x1b[?62;1;2;6;9;15;22c"),
            Some(NegotiationSequence::DeviceAttributes)
        );
        assert_eq!(
            parse_negotiation_sequence("\x1b[?c"),
            Some(NegotiationSequence::DeviceAttributes),
            "Pi's `[\\d;]*` accepts an empty parameter list"
        );
    }

    #[test]
    fn rejects_everything_else() {
        assert_eq!(parse_negotiation_sequence("\x1b[?u"), None, "the QUERY is not a report");
        assert_eq!(parse_negotiation_sequence("\x1b[?997;2n"), None, "a color-scheme report");
        assert_eq!(parse_negotiation_sequence("\x1b[1u"), None, "no `?` — not a flags report");
        assert_eq!(parse_negotiation_sequence("\x1b[?1;2u"), None, "Pi's `\\d+` allows no `;`");
        assert_eq!(parse_negotiation_sequence("hello"), None);
    }

    #[test]
    fn recognizes_still_arriving_prefixes() {
        assert!(is_negotiation_prefix("\x1b["));
        assert!(is_negotiation_prefix("\x1b[?"));
        assert!(is_negotiation_prefix("\x1b[?62;1"));
        assert!(!is_negotiation_prefix("\x1b[?1u"), "a complete frame is not a prefix");
        assert!(!is_negotiation_prefix("a"));
    }

    #[test]
    fn flags_are_found_alongside_the_sentinel_answer() {
        // What a Kitty-protocol terminal sends back for `CSI > 1 u` + `CSI ? u` + `CSI c`.
        assert_eq!(find_kitty_flags("\x1b[?1u\x1b[?62;c"), Some(1));
        // A terminal that reports the flags AFTER its DA1 answer is still read correctly.
        assert_eq!(find_kitty_flags("\x1b[?62;c\x1b[?7u"), Some(7));
        assert_eq!(find_kitty_flags("\x1b[?62;1;2;6;9;15;22c"), None, "DA1 only");
        assert_eq!(find_kitty_flags(""), None);
    }

    #[test]
    fn decision_table_matches_pis_handler() {
        // `flags !== 0` ⇒ Kitty is live (`terminal.ts:233-240`).
        assert_eq!(decide("\x1b[?1u\x1b[?62;c"), KeyboardProtocol::Kitty);
        assert_eq!(decide("\x1b[?7u\x1b[?62;c"), KeyboardProtocol::Kitty);
        // `flags === 0` ⇒ Pi's `enableModifyOtherKeys` branch (`:241`).
        assert_eq!(decide("\x1b[?0u\x1b[?62;c"), KeyboardProtocol::Legacy);
        // DA1 with no flags report ⇒ the same branch, via `:246-248`.
        assert_eq!(decide("\x1b[?62;1;2;6;9;15;22c"), KeyboardProtocol::Legacy);
        // Silence ⇒ nothing is known (Pi simply never fires its handler).
        assert_eq!(decide(""), KeyboardProtocol::Unknown);
        assert_eq!(decide("\x1b]11;rgb:2828/2828/2828\x07"), KeyboardProtocol::Unknown);
    }

    #[test]
    fn a_keystroke_typed_during_startup_does_not_forge_a_report() {
        // Bytes that merely contain `u`/`c` must not be read as a negotiation reply.
        assert_eq!(decide("cursor"), KeyboardProtocol::Unknown);
        assert_eq!(find_kitty_flags("\x1b[?abcu"), None);
    }

    #[test]
    fn a_dead_terminal_is_skipped_not_awaited() {
        // Under the test harness stdin is not a raw-mode tty, so the live probe short-circuits
        // before writing anything: it must return immediately with nothing known.
        let started = std::time::Instant::now();
        assert_eq!(negotiate(), KeyboardProtocol::Unknown);
        assert!(started.elapsed() < Duration::from_secs(1), "the probe must not block");
        // …and the recorded state is readable afterwards.
        assert_eq!(current(), KeyboardProtocol::Unknown);
    }
}
