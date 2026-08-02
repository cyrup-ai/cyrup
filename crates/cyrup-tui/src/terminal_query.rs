//! Terminal background / color-scheme **queries** — the port of Pi's two escape-sequence probes and
//! their reply parsers (`tui/src/tui.ts:1174-1220`, `tui/src/terminal-colors.ts`).
//!
//! Pi asks the terminal two questions before it picks a theme:
//!
//! * **OSC 11** — `ESC ] 11 ; ? BEL` (`queryTerminalBackgroundColor`, `tui.ts:1174-1194`). The reply
//!   is `ESC ] 11 ; <color> BEL` (or `ST`-terminated), where `<color>` is `#rrggbb`,
//!   `#rrrrggggbbbb`, or `rgb:RRRR/GGGG/BBBB` — [`parse_osc11_background_color`].
//! * **DSR `?996`** — `ESC [ ? 996 n` (`queryTerminalColorScheme`, `tui.ts:1202-1220`). A terminal
//!   that implements the color-palette notification protocol replies `CSI ? 997 ; 1 n` (dark) or
//!   `CSI ? 997 ; 2 n` (light) — [`parse_color_scheme_report`]. Note the **query** is `996` and the
//!   **report** is `997`.
//!
//! Both probes are optional: `COLORFGBG` remains the fallback ([`TerminalTheme::detect`]) exactly as
//! in Pi's `catch` arms (`theme.ts:783-787`, `:797-800`).
//!
//! # Why this is not routed through crossterm
//!
//! crossterm 0.29 has no OSC/DSR event: `parse_event` (`event/sys/unix/parse.rs:26`) forwards
//! `ESC ]` to the Alt-key path and rejects `CSI ? … n`, so a reply that reaches its parser is
//! decoded as garbage keystrokes, not surfaced. And `event::poll` cannot be used as a mere readiness
//! check — it drains bytes into that same parser. The probe therefore talks to the fd directly.
//!
//! # Not hanging, and not eating the user's typing (the two hard requirements)
//!
//! 1. **Every read is bounded.** [`StdinTerminalProbe`] never issues a blocking read: it `poll(2)`s
//!    with the deadline's *remaining* time and only `read(2)`s once readiness is reported. A
//!    terminal that answers nothing costs exactly `timeout` (Pi uses 100 ms) and consumes zero
//!    bytes — nothing is left half-read and no thread is left parked on stdin.
//! 2. **A sentinel bounds it in the common case.** Each query is followed by a Primary Device
//!    Attributes request (`ESC [ c`), which every VT-class terminal answers. Replies come back in
//!    order, so seeing the DA1 reply proves the OSC/DSR answer (if the terminal had one) has already
//!    arrived — [`saw_device_attributes`] ends the read immediately instead of idling to the
//!    deadline. This is also what keeps a *late* reply from corrupting input: the sentinel reply is
//!    consumed here rather than surfacing to crossterm later.
//! 3. **It runs in the one safe window.** The probe is issued after raw mode is on
//!    (`App::into_stdout`) and *before* the crossterm reader thread exists
//!    (`crossterm_input_stream`), so there is no second reader to race for the bytes. Both
//!    preconditions are re-checked here ([`stdin_is_queryable`]); if either fails the probe is
//!    skipped and the caller falls back to `COLORFGBG`. (crossterm reads `/dev/tty` where it can
//!    rather than fd 0, but both name the same terminal input queue, so consuming the reply on fd 0
//!    before that reader exists is correct — and `std::io::Stdin`'s own buffer is still untouched at
//!    this point, so nothing is stranded inside it either.)
//!
//! Residual risk, stated plainly: a terminal that answers OSC 11 but *not* DA1, more than `timeout`
//! late, will have its reply reach crossterm and be decoded as stray key events. Pi has the same
//! exposure (its `setTimeout` also just gives up). The window is one 100 ms boot-time slice.

use std::time::Duration;

use crate::theme::TerminalTheme;

/// `ESC ] 11 ; ? BEL` — Pi's OSC 11 background-color query (`tui.ts:1193`).
pub const OSC11_BACKGROUND_QUERY: &str = "\x1b]11;?\x07";

/// `CSI ? 996 n` — Pi's color-scheme DSR (`tui.ts:1219`).
pub const COLOR_SCHEME_QUERY: &str = "\x1b[?996n";

/// `CSI c` — Primary Device Attributes, appended to every query as the ordering sentinel (see the
/// module docs). Not a Pi sequence: Pi's event-loop lets it settle a promise instead.
const DEVICE_ATTRIBUTES_QUERY: &str = "\x1b[c";

/// Hard cap on how much a reply may be, so a chatty/garbage terminal cannot make the boot probe spin.
const MAX_REPLY_BYTES: usize = 1024;

/// The two terminal questions, behind a trait so the theme layer can be driven from a scripted reply
/// in tests (Pi's `TerminalBackgroundThemeDetector` / `TerminalAutoThemeDetector` interfaces,
/// `theme.ts:703-709`).
pub trait TerminalProbe {
    /// OSC 11 → the terminal's default background color, or `None` on timeout / unparseable reply.
    fn query_background_color(&self, timeout: Duration) -> Option<(u8, u8, u8)>;

    /// DSR `?996` → the terminal's declared color scheme. Optional upstream (`queryTerminalColorScheme?`,
    /// `theme.ts:708`), so the default is "unsupported".
    fn query_color_scheme(&self, timeout: Duration) -> Option<TerminalTheme> {
        let _ = timeout;
        None
    }
}

/// A probe that answers nothing — the safe stand-in for a non-tty front-end (print/RPC mode) and the
/// `None`-everything baseline in tests. Detection falls through to `COLORFGBG`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTerminalProbe;

impl TerminalProbe for NoTerminalProbe {
    fn query_background_color(&self, _timeout: Duration) -> Option<(u8, u8, u8)> {
        None
    }
}

// ============================================================================
// Reply parsing (Pi terminal-colors.ts) — pure, total, panic-free
// ============================================================================

/// Pi `parseOsc11BackgroundColor` (`terminal-colors.ts:35-65`) over an exact reply frame:
/// `ESC ] 11 ; <value> (BEL | ESC \)`, where `<value>` is `#rrggbb`, `#rrrrggggbbbb`, or an
/// `rgb:` / `rgba:` triple of arbitrary-width hex channels. Anything else ⇒ `None`.
pub fn parse_osc11_background_color(data: &str) -> Option<(u8, u8, u8)> {
    let body = data.strip_prefix("\x1b]11;")?;
    let value = match body.strip_suffix('\x07') {
        Some(v) => v,
        None => body.strip_suffix("\x1b\\")?,
    };
    // Pi's character class `[^\x07\x1b]*` — no embedded terminator may hide inside the payload.
    if value.contains('\x07') || value.contains('\x1b') {
        return None;
    }
    parse_osc11_value(value.trim())
}

/// The `<value>` half of an OSC 11 reply.
fn parse_osc11_value(value: &str) -> Option<(u8, u8, u8)> {
    if let Some(hex) = value.strip_prefix('#') {
        // `#rrggbb` (8-bit channels) and `#rrrrggggbbbb` (16-bit channels) — `:41-52`.
        return match hex.len() {
            6 => Some((
                parse_osc_hex_channel(hex.get(0..2)?)?,
                parse_osc_hex_channel(hex.get(2..4)?)?,
                parse_osc_hex_channel(hex.get(4..6)?)?,
            )),
            12 => Some((
                parse_osc_hex_channel(hex.get(0..4)?)?,
                parse_osc_hex_channel(hex.get(4..8)?)?,
                parse_osc_hex_channel(hex.get(8..12)?)?,
            )),
            _ => None,
        };
    }
    // `rgb:RRRR/GGGG/BBBB` (xterm's canonical answer) and the `rgba:` variant — `:54-64`.
    let triple = strip_ascii_prefix(value, "rgba:")
        .or_else(|| strip_ascii_prefix(value, "rgb:"))
        .unwrap_or(value);
    let mut parts = triple.split('/');
    let (r, g, b) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((parse_osc_hex_channel(r)?, parse_osc_hex_channel(g)?, parse_osc_hex_channel(b)?))
}

/// Pi `parseOscHexChannel` (`terminal-colors.ts:17-25`): an arbitrary-width hex channel scaled onto
/// `0..=255` by its own maximum (`16^len - 1`), so `ffff` and `ff` both mean full intensity.
fn parse_osc_hex_channel(channel: &str) -> Option<u8> {
    // `16^len - 1` must stay exactly representable: cap the width (xterm emits 1–4 digits).
    if channel.is_empty() || channel.len() > 8 || !channel.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u64::from_str_radix(channel, 16).ok()?;
    let max = 16u64.checked_pow(u32::try_from(channel.len()).ok()?)?.checked_sub(1)?;
    if max == 0 {
        return None;
    }
    // Pi's `Math.round((v / max) * 255)`.
    let scaled = (value.saturating_mul(255).saturating_mul(2).saturating_add(max)) / (max * 2);
    u8::try_from(scaled.min(255)).ok()
}

/// Pi `parseTerminalColorSchemeReport` (`terminal-colors.ts:67-73`) over an exact `CSI ? 997 ; N n`
/// frame: `2` ⇒ light, `1` ⇒ dark. Any other shape ⇒ `None`.
pub fn parse_color_scheme_report(data: &str) -> Option<TerminalTheme> {
    let n = data.strip_prefix("\x1b[?997;")?.strip_suffix('n')?;
    match n {
        "1" => Some(TerminalTheme::Dark),
        "2" => Some(TerminalTheme::Light),
        _ => None,
    }
}

/// Locate an OSC 11 reply anywhere inside a raw read (which may also carry the DA1 sentinel reply)
/// and parse it. Pi anchors its regex because its dispatcher already split the chunk per sequence;
/// here the bytes arrive unsplit, so we scan.
pub fn find_osc11_background_color(buffer: &str) -> Option<(u8, u8, u8)> {
    let start = buffer.find("\x1b]11;")?;
    let rest = buffer.get(start..)?;
    // The frame ends at the first BEL or ST after the introducer.
    let bel = rest.find('\x07');
    let st = rest.get(2..).and_then(|r| r.find("\x1b\\")).map(|i| i + 2);
    let (end, term_len) = match (bel, st) {
        (Some(b), Some(s)) if b < s => (b, 1),
        (Some(_), Some(s)) => (s, 2),
        (Some(b), None) => (b, 1),
        (None, Some(s)) => (s, 2),
        (None, None) => return None,
    };
    parse_osc11_background_color(rest.get(..end + term_len)?)
}

/// Locate a `CSI ? 997 ; N n` report anywhere inside a raw read and parse it.
pub fn find_color_scheme_report(buffer: &str) -> Option<TerminalTheme> {
    let start = buffer.find("\x1b[?997;")?;
    let rest = buffer.get(start..)?;
    let end = rest.find('n')?;
    parse_color_scheme_report(rest.get(..=end)?)
}

/// Whether the buffer already holds a Device-Attributes reply (`CSI … c`) — the ordering sentinel
/// that says "everything the terminal was going to send for this query has been sent".
pub fn saw_device_attributes(buffer: &[u8]) -> bool {
    let mut i = 0usize;
    while let Some(&b) = buffer.get(i) {
        if b == 0x1b && buffer.get(i + 1) == Some(&b'[') {
            // Skip parameter + intermediate bytes; a CSI's final byte is 0x40..=0x7e.
            let mut j = i + 2;
            loop {
                match buffer.get(j) {
                    Some(&p) if (0x40..=0x7e).contains(&p) => {
                        if p == b'c' {
                            return true;
                        }
                        break;
                    }
                    Some(_) => j += 1,
                    // The sequence is still arriving — not the sentinel *yet*, keep reading.
                    None => return false,
                }
            }
            // Some other CSI (e.g. the `?997` color-scheme report, which precedes the sentinel):
            // step past it and keep looking.
            i = j.saturating_add(1);
            continue;
        }
        i += 1;
    }
    false
}

/// Case-insensitive ASCII `strip_prefix`.
fn strip_ascii_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let head = value.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| value.get(prefix.len()..))?
}

// ============================================================================
// The live probe
// ============================================================================

/// The production probe: writes the query to stdout and reads the reply straight off stdin under a
/// hard deadline. See the module docs for the timeout / input-safety contract.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdinTerminalProbe;

impl TerminalProbe for StdinTerminalProbe {
    fn query_background_color(&self, timeout: Duration) -> Option<(u8, u8, u8)> {
        find_osc11_background_color(&exchange(OSC11_BACKGROUND_QUERY, timeout)?)
    }

    fn query_color_scheme(&self, timeout: Duration) -> Option<TerminalTheme> {
        find_color_scheme_report(&exchange(COLOR_SCHEME_QUERY, timeout)?)
    }
}

/// Both preconditions for a query to be answerable at all: stdin/stdout are a terminal, and raw mode
/// is on (in cooked mode the reply sits in the line discipline until the user presses Enter, which
/// would both hang the probe and hand the escape bytes to the shell).
fn stdin_is_queryable() -> bool {
    use ratatui::crossterm::terminal::is_raw_mode_enabled;
    use ratatui::crossterm::tty::IsTty;
    std::io::stdin().is_tty() && std::io::stdout().is_tty() && is_raw_mode_enabled().unwrap_or(false)
}

/// Write `request` (plus the DA1 sentinel) and collect whatever comes back within `timeout`.
fn exchange(request: &str, timeout: Duration) -> Option<String> {
    use std::io::Write;
    if !stdin_is_queryable() {
        return None;
    }
    let mut out = std::io::stdout();
    out.write_all(request.as_bytes()).ok()?;
    out.write_all(DEVICE_ATTRIBUTES_QUERY.as_bytes()).ok()?;
    out.flush().ok()?;
    read_reply(timeout)
}

/// Read from stdin until the DA1 sentinel arrives, the byte cap is hit, or the deadline expires.
/// Never blocks past the deadline and never consumes a byte the terminal did not send in reply.
#[cfg(unix)]
fn read_reply(timeout: Duration) -> Option<String> {
    use std::time::Instant;

    use rustix::event::{PollFd, PollFlags, Timespec};
    use rustix::io::Errno;

    let stdin = std::io::stdin();
    let deadline = Instant::now().checked_add(timeout)?;
    let mut collected: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 128];
    while collected.len() < MAX_REPLY_BYTES {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let ts = Timespec {
            tv_sec: i64::try_from(remaining.as_secs()).unwrap_or(i64::MAX) as _,
            tv_nsec: i64::from(remaining.subsec_nanos()) as _,
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&ts)) {
            // Timed out: the terminal does not answer this query. Nothing was consumed.
            Ok(0) => break,
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(_) => break,
        }
        match rustix::io::read(&stdin, &mut chunk[..]) {
            Ok(0) => break,
            Ok(n) => collected.extend_from_slice(chunk.get(..n).unwrap_or_default()),
            Err(Errno::INTR) | Err(Errno::AGAIN) => continue,
            Err(_) => break,
        }
        if saw_device_attributes(&collected) {
            break;
        }
    }
    (!collected.is_empty()).then(|| String::from_utf8_lossy(&collected).into_owned())
}

/// Non-Unix has no `poll`-able stdin fd here; detection falls back to `COLORFGBG`.
#[cfg(not(unix))]
fn read_reply(_timeout: Duration) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    #[test]
    fn osc11_hex_forms() {
        assert_eq!(parse_osc11_background_color("\x1b]11;#1e1e1e\x07"), Some((30, 30, 30)));
        // 16-bit channels scale down (`ffff` ⇒ 255), and ST terminates as well as BEL.
        assert_eq!(
            parse_osc11_background_color("\x1b]11;#ffffffffffff\x1b\\"),
            Some((255, 255, 255))
        );
    }

    #[test]
    fn osc11_rgb_forms() {
        assert_eq!(
            parse_osc11_background_color("\x1b]11;rgb:ffff/ffff/ffff\x07"),
            Some((255, 255, 255))
        );
        assert_eq!(parse_osc11_background_color("\x1b]11;rgb:00/00/00\x07"), Some((0, 0, 0)));
        // xterm's half-intensity `8080` rounds to 128, matching Pi's `Math.round`.
        assert_eq!(
            parse_osc11_background_color("\x1b]11;rgb:8080/8080/8080\x07"),
            Some((128, 128, 128))
        );
        assert_eq!(parse_osc11_background_color("\x1b]11;RGBA:ff/00/00\x07"), Some((255, 0, 0)));
    }

    #[test]
    fn osc11_rejects_malformed_replies() {
        assert_eq!(parse_osc11_background_color("\x1b]11;#zzzzzz\x07"), None);
        assert_eq!(parse_osc11_background_color("\x1b]11;#12345\x07"), None);
        assert_eq!(parse_osc11_background_color("\x1b]11;rgb:ff/00\x07"), None);
        assert_eq!(parse_osc11_background_color("\x1b]10;#ffffff\x07"), None, "OSC 10 is not 11");
        assert_eq!(parse_osc11_background_color("\x1b]11;#ffffff"), None, "unterminated");
    }

    #[test]
    fn color_scheme_report_forms() {
        assert_eq!(parse_color_scheme_report("\x1b[?997;1n"), Some(TerminalTheme::Dark));
        assert_eq!(parse_color_scheme_report("\x1b[?997;2n"), Some(TerminalTheme::Light));
        assert_eq!(parse_color_scheme_report("\x1b[?997;3n"), None);
        assert_eq!(parse_color_scheme_report("\x1b[?996n"), None, "the QUERY is not a report");
    }

    #[test]
    fn replies_are_found_alongside_the_sentinel_answer() {
        // What a real xterm sends back for `OSC 11 ? BEL` + `CSI c`, in one read.
        let raw = "\x1b]11;rgb:2828/2828/2828\x07\x1b[?62;1;2;6;9;15;22c";
        assert_eq!(find_osc11_background_color(raw), Some((40, 40, 40)));
        assert!(saw_device_attributes(raw.as_bytes()));

        let raw = "\x1b[?997;2n\x1b[?62;c";
        assert_eq!(find_color_scheme_report(raw), Some(TerminalTheme::Light));
        assert!(saw_device_attributes(raw.as_bytes()));
    }

    #[test]
    fn sentinel_detection_waits_for_the_final_byte() {
        // A DA1 reply still arriving in pieces must NOT end the read early.
        assert!(!saw_device_attributes(b"\x1b[?62;1;2"));
        assert!(saw_device_attributes(b"\x1b[?62;1;2c"));
        // A different CSI final byte is not the sentinel.
        assert!(!saw_device_attributes(b"\x1b[?997;2n"));
        assert!(!saw_device_attributes(b"no escapes here"));
    }

    #[test]
    fn a_dead_terminal_is_skipped_not_awaited() {
        // In the test harness stdin is not a raw-mode tty, so the live probe short-circuits before
        // writing anything — it must return immediately, never park on a read.
        let started = std::time::Instant::now();
        assert_eq!(StdinTerminalProbe.query_background_color(Duration::from_secs(30)), None);
        assert_eq!(StdinTerminalProbe.query_color_scheme(Duration::from_secs(30)), None);
        assert!(started.elapsed() < Duration::from_secs(1), "the probe must not block");
    }
}
