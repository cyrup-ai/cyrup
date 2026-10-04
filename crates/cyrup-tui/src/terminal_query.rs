//! Terminal **queries** — the port of Pi's escape-sequence probes and their reply parsers
//! (`tui/src/tui.ts`, `tui/src/terminal-colors.ts` @v1.0.0).
//!
//! Pi asks the terminal two things at boot — one before it picks a theme, one before it lays out an
//! image:
//!
//! * **The terminal's colours** — `OSC 10 ; ?`, `OSC 11 ; ?` and sixteen `OSC 4 ; <i> ; ?`, then a
//!   Primary Device Attributes request (`TERMINAL_COLOR_QUERY`, `tui.ts:168-171`). Replies are
//!   `ESC ] 10 ; <color> BEL`, `ESC ] 11 ; <color> BEL` and `ESC ] 4 ; <i> ; <color> BEL` (each
//!   `ST`-terminated as well), where `<color>` is `#rrggbb`, `#rrrrggggbbbb`, or
//!   `rgb:RRRR/GGGG/BBBB` — [`parse_osc_color_response`]. The answer is a [`TerminalColors`]:
//!   foreground, background, and the 16-colour palette only when the terminal reported ALL sixteen.
//! * **`CSI 16 t`** — the cell-size query (`queryCellSize`, `tui.ts:679-686`), asked only of a
//!   terminal that has an image protocol. The reply is `CSI 6 ; height ; width t` —
//!   [`parse_cell_size_report`] — and it is what stops inline images being laid out against a
//!   guessed font cell ([`crate::image::ImageRenderer::from_capabilities_with_cell_size`]).
//!
//! Pi v1.0.0 DELETED `queryTerminalBackgroundColor` and `queryTerminalColorScheme`; the light/dark
//! report now arrives only as a mode-`2031` notification ([`parse_color_scheme_report`]), so there
//! is no `CSI ? 996 n` query here any more.
//!
//! A third query is cyrup-original, not a Pi port — Pi's hand-rolled renderer never needs it, but
//! ratatui's inline viewport does:
//!
//! * **DSR `CSI 6 n`** — the cursor-position report (`find_cursor_position_report`). Asked EXACTLY
//!   ONCE, at boot, through the same bounded `exchange` as the probes above, to seed
//!   [`crate::InlineBackend`]'s tracked cursor anchor (TUI-093). After that single call cyrup never
//!   asks the terminal where the cursor is again — the backend answers from the anchor it already
//!   knows, because `crossterm::cursor::position()` would otherwise race
//!   [`crate::app::crossterm_input_stream`]'s permanent reader thread on every commit flush, window
//!   resize and live-region height change, which is what produced the crash this exists to fix.
//!
//! Every query is optional: `COLORFGBG` remains the fallback ([`TerminalTheme::detect`]) exactly as
//! in Pi's `detectTerminalTheme` (`theme.ts:702-710`).
//!
//! # Who owns the next Primary Device Attributes reply
//!
//! Every query here ends in a DA1 request, because every VT-class terminal answers it and answers
//! in order: seeing the DA1 reply proves the OSC/DSR answer (if the terminal had one) has already
//! arrived. With more than one query, a DA1 reply only means something once it is attributed to the
//! query that asked — pi's keyboard-protocol negotiation counts the DA1 replies it is owed
//! (`pendingKeyboardProtocolDeviceAttributes`, `terminal.ts:144`, `:262`, `:271-273`) and FORWARDS
//! any further one, because the colour query's trailing DA1 is a second consumer
//! (`tui.ts:1122-1130`, where the first pending colour query takes the DA1 and completes). Here the
//! [`Hub`] holds the ledger: owners are queued in the order they wrote their request, a DA1 reply
//! pops the front, and a query that gave up keeps its place so its late reply is not taken for the
//! next query's.
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
//! 2. **A sentinel bounds it in the common case.** Seeing the DA1 reply — or, for the colour query,
//!    all eighteen colours — ends the read immediately instead of idling to the deadline.
//! 3. **It runs in the one safe window.** The probe is issued after raw mode is on
//!    (`App::into_stdout`) and *before* the input reader thread exists
//!    (`crossterm_input_stream`), so there is no second reader to race for the bytes. Both
//!    preconditions are re-checked here ([`stdin_is_queryable`]); if either fails the probe is
//!    skipped and the caller falls back to `COLORFGBG`.
//!
//! # A reply that arrives after the deadline
//!
//! Pi keeps a timed-out colour query in its pending list and hands the finished result to
//! `onLateReply` (`tui.ts:1470-1493`). So does the [`Hub`]: once the reader thread exists, every
//! escape sequence it frames is first offered to [`offer_reply`], and a colour reply or DA1 that
//! belongs to a pending query is absorbed there — completing the query and calling the late
//! callback — instead of reaching the decoder. Anything the hub does not own is decoded as before:
//! the reader frames an OSC, DCS or APC string, a `CSI ?` reply, a cursor-position report and a
//! `CSI … t` window report as one sequence each and swallows them as terminal replies
//! (`crate::input::decode`). On Windows, where crossterm's console reader remains and the probe
//! does not run at all, `crate::stray_reply` filters the shredded OSC reply instead.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use cyrup_resources::color::Rgb;

use crate::theme::TerminalTheme;
use crate::write_log::tui_stdout;

/// `CSI 16 t` — Pi's terminal cell-size query (`tui.ts:679-686`, `queryCellSize`). The reply is
/// `CSI 6 ; <height_px> ; <width_px> t` ([`parse_cell_size_report`]). Pi issues it only when the
/// terminal has an image protocol (`:681`), because the cell size is used for nothing else.
pub const CELL_SIZE_QUERY: &str = "\x1b[16t";

/// The cell-size probe's budget. Pi needs none (its reply is consumed by the input handler whenever
/// it arrives, `tui.ts:791`); cyrup's probe is synchronous, so it is bounded like the colour query —
/// Pi's own 100 ms figure (`theme-controller.ts:25`).
pub const CELL_SIZE_TIMEOUT: Duration = Duration::from_millis(100);

/// `CSI 6 n` — DSR cursor-position report; cyrup-original (TUI-093), not a Pi sequence. The reply is
/// `CSI <row> ; <col> R`, 1-based ([`parse_cursor_position_report`]). Asked ONCE, at boot, to seed
/// [`crate::InlineBackend`]'s anchor; after that cyrup tracks the cursor itself and the terminal is
/// never asked again — see the module docs for why asking on every frame crashed the app.
pub const CURSOR_POSITION_QUERY: &str = "\x1b[6n";

/// The cursor-position probe's budget — the same 100 ms figure as the other probes.
pub const CURSOR_POSITION_TIMEOUT: Duration = Duration::from_millis(100);

/// How long the colour query waits — Pi `TERMINAL_QUERY_TIMEOUT_MS` (`theme-controller.ts:25`).
/// Terminals answer the trailing DA1 right after the colour replies, so this only matters for a
/// terminal that answers neither; replies arriving later still apply.
pub const COLOR_QUERY_TIMEOUT: Duration = Duration::from_millis(100);

/// `CSI c` — Primary Device Attributes, the ordering sentinel every query ends in (see the module
/// docs).
const DEVICE_ATTRIBUTES_QUERY: &str = "\x1b[c";

/// Pi `TERMINAL_PALETTE_SIZE` (`tui.ts:159`).
const TERMINAL_PALETTE_SIZE: usize = 16;

/// Pi `TERMINAL_COLOR_REPLY_COUNT` (`tui.ts:162`): OSC 10 and 11 plus OSC 4 for every palette colour.
const TERMINAL_COLOR_REPLY_COUNT: usize = 2 + TERMINAL_PALETTE_SIZE;

/// Pi `TERMINAL_COLOR_QUERY` (`tui.ts:168-171`): the default colours, palette colours 0-15, and a
/// trailing DA1 request that marks the end of the replies — including for terminals that ignore the
/// colour queries.
pub static TERMINAL_COLOR_QUERY: LazyLock<String> = LazyLock::new(|| {
    let mut query = String::from("\x1b]10;?\x07\x1b]11;?\x07");
    for index in 0..TERMINAL_PALETTE_SIZE {
        query.push_str(&format!("\x1b]4;{index};?\x07"));
    }
    query.push_str(DEVICE_ATTRIBUTES_QUERY);
    query
});

/// Hard cap on how much a reply may be, so a chatty/garbage terminal cannot make the boot probe
/// spin. Eighteen colour replies are about 550 bytes.
#[cfg(unix)]
const MAX_REPLY_BYTES: usize = 4096;

// ============================================================================
// What the terminal reports
// ============================================================================

/// Colours the terminal reports for its current theme — Pi `TerminalColors`
/// (`terminal-colors.ts:10-17`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalColors {
    /// Default foreground (OSC 10).
    pub foreground: Option<Rgb>,
    /// Default background (OSC 11).
    pub background: Option<Rgb>,
    /// ANSI colours 0-15 (OSC 4). Only set when the terminal reported all 16 — the all-or-nothing
    /// rule is the type: a partial palette has no representation.
    pub palette: Option<[Rgb; 16]>,
}

/// What an OSC colour reply reports: the default foreground (OSC 10), background (OSC 11), or a
/// palette index (OSC 4) — Pi `OscColorTarget` (`terminal-colors.ts:39`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OscColorTarget {
    Foreground,
    Background,
    /// `OSC 4 ; <n>` — pi's pattern admits one to three digits, so this can name an index beyond
    /// the sixteen the query asked for.
    Palette(u16),
}

/// A parsed OSC colour reply: `rgb` is `None` when it is a reply whose colour is unparseable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OscColorResponse {
    pub target: OscColorTarget,
    pub rgb: Option<Rgb>,
}

/// Receives a colour query's result after its timeout, when the terminal finally completes it —
/// Pi's `onLateReply`.
pub type LateColors = Arc<dyn Fn(TerminalColors) + Send + Sync>;

/// The terminal questions, behind a trait so the theme layer can be driven from a scripted reply in
/// tests.
pub trait TerminalProbe {
    /// The terminal's colours — Pi `queryTerminalColors({ timeoutMs, onLateReply })`
    /// (`tui.ts:478`, `:1470`). Resolves on the DA1 reply or once all eighteen colours answered, or
    /// at `timeout`; whatever the terminal did not report is `None`. If the query has not completed
    /// by `timeout`, `on_late_reply` receives the finished result if it completes later.
    ///
    /// Optional like the other probes: a terminal that stays silent, or a front-end with no
    /// terminal, reports nothing.
    fn query_terminal_colors(
        &self,
        timeout: Duration,
        on_late_reply: Option<LateColors>,
    ) -> TerminalColors {
        let _ = (timeout, on_late_reply);
        TerminalColors::default()
    }

    /// `CSI 16 t` → the terminal's cell size in pixels as `(width, height)` — Pi `queryCellSize`
    /// (`tui.ts:679-686`) feeding `setCellDimensions` (`:890`). Optional: a terminal that does not
    /// answer leaves the image layer on its default cell (`terminal-image.ts:37`'s
    /// `{widthPx: 9, heightPx: 18}`; ratatui-image's is `10x20`).
    fn query_cell_size(&self, timeout: Duration) -> Option<(u16, u16)> {
        let _ = timeout;
        None
    }

    /// `CSI 6 n` → the cursor's 0-based `(col, row)`. cyrup-original (TUI-093), not a Pi port: seeds
    /// [`crate::InlineBackend`]'s anchor at boot so the terminal is never queried for its cursor
    /// again. Optional like the probes above: a terminal that stays silent leaves the caller on its
    /// own fallback (the live viewport's bottom row).
    fn query_cursor_position(&self, timeout: Duration) -> Option<(u16, u16)> {
        let _ = timeout;
        None
    }
}

/// A probe that answers nothing — the safe stand-in for a non-tty front-end (print/RPC mode) and the
/// `None`-everything baseline in tests. Detection falls through to `COLORFGBG`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTerminalProbe;

impl TerminalProbe for NoTerminalProbe {}

// ============================================================================
// Reply parsing (Pi terminal-colors.ts) — pure, total, panic-free
// ============================================================================

/// Pi `parseOscColorResponse` (`terminal-colors.ts:48-56`) over an exact reply frame — the single
/// pattern `/^\x1b\](?:(1[01])|4;(\d{1,3}));([^\x07\x1b]*)(?:\x07|\x1b\\)$/i`: OSC 10, 11, or
/// `4 ; <index>`, a `;`, a payload with no embedded terminator, and `BEL` or `ST`. Anything else ⇒
/// `None`; a reply whose colour does not parse is `Some` with `rgb: None`.
#[must_use]
pub fn parse_osc_color_response(data: &str) -> Option<OscColorResponse> {
    let body = data.strip_prefix("\x1b]")?;
    let body = body
        .strip_suffix('\x07')
        .or_else(|| body.strip_suffix("\x1b\\"))?;
    let (target, value) = if let Some(rest) = body.strip_prefix("10;") {
        (OscColorTarget::Foreground, rest)
    } else if let Some(rest) = body.strip_prefix("11;") {
        (OscColorTarget::Background, rest)
    } else {
        let rest = body.strip_prefix("4;")?;
        let (digits, value) = rest.split_once(';')?;
        // `\d{1,3}`, ASCII digits only.
        if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        (OscColorTarget::Palette(digits.parse().ok()?), value)
    };
    // `[^\x07\x1b]*` — no terminator may hide inside the payload.
    if value.contains('\x07') || value.contains('\x1b') {
        return None;
    }
    Some(OscColorResponse {
        target,
        rgb: parse_osc_color_value(value),
    })
}

/// Pi `parseOscColorValue` (`terminal-colors.ts:58-83`): `#rrggbb`, `#rrrrggggbbbb`, or an `rgb:` /
/// `rgba:` triple of arbitrary-width hex channels. Surrounding whitespace is tolerated; a fourth
/// `/`-separated component (an alpha) is ignored, as pi's destructuring ignores it.
fn parse_osc_color_value(raw: &str) -> Option<Rgb> {
    let value = raw.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            6 => Some(Rgb::new(
                parse_osc_hex_channel(hex.get(0..2)?)?,
                parse_osc_hex_channel(hex.get(2..4)?)?,
                parse_osc_hex_channel(hex.get(4..6)?)?,
            )),
            12 => Some(Rgb::new(
                parse_osc_hex_channel(hex.get(0..4)?)?,
                parse_osc_hex_channel(hex.get(4..8)?)?,
                parse_osc_hex_channel(hex.get(8..12)?)?,
            )),
            _ => None,
        };
    }
    // `value.replace(/^rgba?:/i, "")`.
    let triple = strip_ascii_prefix(value, "rgba:")
        .or_else(|| strip_ascii_prefix(value, "rgb:"))
        .unwrap_or(value);
    let mut parts = triple.split('/');
    let (r, g, b) = (parts.next()?, parts.next()?, parts.next()?);
    Some(Rgb::new(
        parse_osc_hex_channel(r)?,
        parse_osc_hex_channel(g)?,
        parse_osc_hex_channel(b)?,
    ))
}

/// Pi `parseOscHexChannel` (`terminal-colors.ts:27-36`): an arbitrary-width hex channel scaled onto
/// `0..=255` by its own maximum (`16^len - 1`), so `ffff` and `ff` both mean full intensity.
fn parse_osc_hex_channel(channel: &str) -> Option<u8> {
    if channel.is_empty() || !channel.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    // `parseInt(channel, 16)` and `16 ** channel.length - 1`, in the same doubles pi uses.
    let value = channel.bytes().fold(0.0_f64, |acc, b| {
        acc * 16.0 + f64::from(char::from(b).to_digit(16).unwrap_or(0))
    });
    let max = 16.0_f64.powi(i32::try_from(channel.len()).ok()?) - 1.0;
    if max <= 0.0 {
        return None;
    }
    // Pi's `Math.round((v / max) * 255)`; `value <= max`, so the result is in range.
    let scaled = ((value / max) * 255.0).round();
    (0.0..=255.0).contains(&scaled).then_some(scaled as u8)
}

/// Pi `parseTerminalColorSchemeReport` (v0.84.1 `tui/src/terminal-colors.ts:67-73`) over a run of
/// **one or more back-to-back** `CSI ? 997 ; N n` frames: `2` ⇒ light, `1` ⇒ dark.
///
/// Pi's pattern is `/^(?:\x1b\[\?997;(1|2)n)+$/` (v0.84.1 `terminal-colors.ts:29`). Two properties
/// of that regex are load-bearing and are reproduced exactly here:
///
/// 1. **The `+` makes a burst legal, and the capture group keeps the LAST iteration** — a terminal
///    that emits several reports in one write (theme flipped twice, or a duplicate report chasing
///    the first) settles on the newest, not the stalest. Pi pins this at
///    `terminal-colors.test.ts:118-119`: `"…;2n…;1n…;1n"` ⇒ dark, `"…;1n…;2n…;2n"` ⇒ light.
/// 2. **It stays anchored `^…$`** — anything that is not a whole number of well-formed frames
///    fails the *entire* match, so one malformed or truncated frame poisons the burst rather than
///    the parser falling back on its healthy neighbours (`terminal-colors.test.ts:120-122`:
///    `"…;3n"`, `"\x1b[?996n"` and `"x\x1b[?997;1n"` are all `undefined`).
///
/// **Version lag, not a port bug.** At v0.83.0 the pattern was the single-frame
/// `/^\x1b\[\?997;(1|2)n$/`; upstream widened it in `0e633790c` ("fix(tui): handle batched color
/// scheme reports", #7550). The pre-batch port was faithful to the tag it was written against.
pub fn parse_color_scheme_report(data: &str) -> Option<TerminalTheme> {
    let mut rest = data;
    let mut last = None;
    // Pi's `(?:…)+` — consume frames left to right; the capture (here `last`) ends up holding the
    // final iteration's value.
    while let Some(after_introducer) = rest.strip_prefix("\x1b[?997;") {
        // A frame with no terminator is a truncated report: Pi's anchor rejects the whole string.
        let (n, tail) = after_introducer.split_once('n')?;
        last = Some(match n {
            "1" => TerminalTheme::Dark,
            "2" => TerminalTheme::Light,
            _ => return None,
        });
        rest = tail;
    }
    // Pi's `$`: leftover bytes (or, with `last == None`, leading bytes) mean no match at all.
    // An empty `data` also yields `None`, because `+` demands at least one frame.
    if rest.is_empty() { last } else { None }
}

/// Pi `consumeCellSizeResponse` (`tui.ts:877-890`) over an exact `CSI 6 ; <height> ; <width> t`
/// frame, returning `(width_px, height_px)` — note the reply's order is height-then-width and Pi's
/// `setCellDimensions({widthPx, heightPx})` swaps them back.
///
/// Pi's `heightPx <= 0 || widthPx <= 0` guard (`:885`) becomes a `None` here rather than Pi's
/// "consumed but ignored": a zero cell would divide the image geometry by zero.
pub fn parse_cell_size_report(data: &str) -> Option<(u16, u16)> {
    let body = data.strip_prefix("\x1b[6;")?.strip_suffix('t')?;
    let (height, width) = body.split_once(';')?;
    // Pi's `(\d+)` — digits only, no sign, no empty field.
    if height.is_empty()
        || width.is_empty()
        || !height.bytes().all(|b| b.is_ascii_digit())
        || !width.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let height: u16 = height.parse().ok()?;
    let width: u16 = width.parse().ok()?;
    (height > 0 && width > 0).then_some((width, height))
}

/// Locate a `CSI 6 ; H ; W t` report anywhere inside a raw read (which also carries the DA1 sentinel
/// reply) and parse it into `(width_px, height_px)`.
pub fn find_cell_size_report(buffer: &str) -> Option<(u16, u16)> {
    let start = buffer.find("\x1b[6;")?;
    let rest = buffer.get(start..)?;
    let end = rest.find('t')?;
    parse_cell_size_report(rest.get(..=end)?)
}

/// `CSI <row> ; <col> R` → 0-based `(col, row)` — the DSR cursor-position report. A leading `?`
/// (DECXCPR) is tolerated but not required, since real terminals answer the plain `CSI 6 n` query
/// with the plain form. Digits-only, non-empty fields, like [`parse_cell_size_report`]; a `0` row or
/// column is rejected via `checked_sub` rather than underflowed — CPR coordinates are 1-based, and
/// a bare `- 1` inside `bool::then_some`'s EAGERLY-evaluated argument would subtract-with-overflow
/// (a debug-build panic) on a malformed `0` from an adversarial or merely buggy terminal, which
/// R-00-009 forbids on any path reachable from terminal input.
pub fn parse_cursor_position_report(data: &str) -> Option<(u16, u16)> {
    let body = data.strip_prefix("\x1b[")?;
    let body = body.strip_prefix('?').unwrap_or(body);
    let body = body.strip_suffix('R')?;
    let (row, col) = body.split_once(';')?;
    if row.is_empty()
        || col.is_empty()
        || !row.bytes().all(|b| b.is_ascii_digit())
        || !col.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let row: u16 = row.parse().ok()?;
    let col: u16 = col.parse().ok()?;
    Some((col.checked_sub(1)?, row.checked_sub(1)?))
}

/// Locate a `CSI <row> ; <col> R` report anywhere inside a raw read (which also carries the DA1
/// sentinel reply) and parse it into 0-based `(col, row)` — the same single-frame scan shape as
/// [`find_cell_size_report`] (only one reply is ever expected: the boot probe issues exactly one
/// `CSI 6 n`).
pub fn find_cursor_position_report(buffer: &str) -> Option<(u16, u16)> {
    let start = buffer.find("\x1b[")?;
    let rest = buffer.get(start..)?;
    let end = rest.find('R')?;
    parse_cursor_position_report(rest.get(..=end)?)
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
            // Some other CSI (e.g. a cursor-position report, which precedes the sentinel): step
            // past it and keep looking.
            i = j.saturating_add(1);
            continue;
        }
        i += 1;
    }
    false
}

/// Pi `DEVICE_ATTRIBUTES_RESPONSE_PATTERN` (`tui.ts:172`): `/^\x1b\[\?[\d;]*c$/`.
fn is_device_attributes_reply(data: &str) -> bool {
    data.strip_prefix("\x1b[?")
        .and_then(|rest| rest.strip_suffix('c'))
        .is_some_and(|params| params.bytes().all(|b| b.is_ascii_digit() || b == b';'))
}

/// Case-insensitive ASCII `strip_prefix`.
fn strip_ascii_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let head = value.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| value.get(prefix.len()..))?
}

// ============================================================================
// One colour query's accumulating state (Pi `PendingTerminalColorQuery`, tui.ts:143-156)
// ============================================================================

/// The replies one colour query has collected so far — pi's `PendingTerminalColorQuery` without its
/// timer and promise. `open` is pi's `deliver !== undefined`: the query has not yet completed.
#[derive(Debug)]
struct TerminalColorQuery {
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    palette: [Option<Rgb>; TERMINAL_PALETTE_SIZE],
    /// Targets that already replied, so duplicates do not count twice.
    replied: HashSet<OscColorTarget>,
    open: bool,
}

impl TerminalColorQuery {
    fn new() -> Self {
        Self {
            foreground: None,
            background: None,
            palette: [None; TERMINAL_PALETTE_SIZE],
            replied: HashSet::new(),
            open: true,
        }
    }

    /// Pi `terminalColorQueryResult` (`tui.ts:1156-1159`): the palette only when all sixteen
    /// arrived.
    fn result(&self) -> TerminalColors {
        let mut palette = [Rgb::new(0, 0, 0); TERMINAL_PALETTE_SIZE];
        let mut complete = true;
        for (slot, color) in palette.iter_mut().zip(self.palette.iter()) {
            match color {
                Some(color) => *slot = *color,
                None => complete = false,
            }
        }
        TerminalColors {
            foreground: self.foreground,
            background: self.background,
            palette: complete.then_some(palette),
        }
    }

    /// Pi `completeTerminalColorQuery` (`tui.ts:1161-1166`): the result, once.
    fn complete(&mut self) -> Option<TerminalColors> {
        if !self.open {
            return None;
        }
        self.open = false;
        Some(self.result())
    }

    /// One OSC colour reply (`consumeTerminalColorResponse`, `tui.ts:1122-1154`). A duplicate, or a
    /// reply after completion, is absorbed without effect. Completes on the eighteenth distinct
    /// target.
    fn on_osc(&mut self, response: OscColorResponse) -> Option<TerminalColors> {
        if !self.open || !self.replied.insert(response.target) {
            return None;
        }
        match response.target {
            OscColorTarget::Foreground => self.foreground = response.rgb,
            OscColorTarget::Background => self.background = response.rgb,
            OscColorTarget::Palette(index) => {
                if let Some(slot) = self.palette.get_mut(usize::from(index)) {
                    *slot = response.rgb;
                }
            }
        }
        if self.replied.len() == TERMINAL_COLOR_REPLY_COUNT {
            self.complete()
        } else {
            None
        }
    }
}

// ============================================================================
// The ledger: who is owed the next DA1 reply, and which colour queries are open
// ============================================================================

/// The party a queued DA1 reply belongs to.
#[derive(Debug)]
enum Owner {
    /// A synchronous [`exchange`], identified by its ticket. `abandoned` once it gave up waiting, so
    /// its late reply is absorbed rather than taken for the next query's.
    Exchange { ticket: u64, abandoned: bool },
    /// A colour query.
    Colors(u64),
}

/// One open colour query, as the ledger tracks it.
struct ColorQuery {
    id: u64,
    machine: TerminalColorQuery,
    /// The completed result, waiting for the thread that is blocked on this query.
    result: Option<TerminalColors>,
    /// Set once the waiter gave up: completion is now delivered to `late` instead.
    timed_out: bool,
    late: Option<LateColors>,
    /// This query's DA1 reply has arrived. Until it does the query stays listed even when all
    /// eighteen colours completed it first, because the reply is still owed (pi leaves the query
    /// at the head of `pendingTerminalColorQueries` for exactly that, `tui.ts:1122-1130`).
    da1_seen: bool,
}

/// What [`Hub::dispatch`] did with one escape sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dispatch {
    /// Not a reply this hub is waiting for; the caller keeps it.
    Forward,
    /// Absorbed by a query, or by a query that already gave up.
    Consumed,
    /// This was the DA1 reply an [`exchange`] was waiting for — `ticket` identifies which.
    Sentinel(u64),
}

/// The late result of a timed-out query, to be handed to its callback once the hub's lock is gone.
pub(crate) type LateDelivery = (LateColors, TerminalColors);

/// Every query in flight, in the order they were written (see the module docs). A plain value so it
/// can be driven from a scripted terminal; production uses the process-wide one behind
/// [`offer_reply`].
#[derive(Default)]
pub(crate) struct Hub {
    next_id: u64,
    owed: VecDeque<Owner>,
    queries: Vec<ColorQuery>,
}

impl Hub {
    fn fresh_id(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    /// Whether nothing is in flight — the reader thread's cheap "skip the lock" test.
    fn is_idle(&self) -> bool {
        self.owed.is_empty() && self.queries.is_empty()
    }

    /// An [`exchange`] is about to write its request and a trailing DA1: queue its claim.
    pub(crate) fn begin_exchange(&mut self) -> u64 {
        let ticket = self.fresh_id();
        self.owed.push_back(Owner::Exchange {
            ticket,
            abandoned: false,
        });
        ticket
    }

    /// The exchange stopped waiting (timeout, byte cap, failed write). Its DA1, if one is still
    /// coming, keeps its place in the queue and is absorbed when it lands.
    pub(crate) fn abandon_exchange(&mut self, ticket: u64) {
        for owner in &mut self.owed {
            if let Owner::Exchange {
                ticket: t,
                abandoned,
            } = owner
                && *t == ticket
            {
                *abandoned = true;
            }
        }
    }

    /// A colour query is about to write [`TERMINAL_COLOR_QUERY`]: open it and queue its DA1.
    pub(crate) fn begin_colors(&mut self) -> u64 {
        let id = self.fresh_id();
        self.queries.push(ColorQuery {
            id,
            machine: TerminalColorQuery::new(),
            result: None,
            timed_out: false,
            late: None,
            da1_seen: false,
        });
        self.owed.push_back(Owner::Colors(id));
        id
    }

    /// Drop query `id` once its DA1 has arrived and nothing is left waiting on its result.
    fn retire_colors(&mut self, id: u64) {
        self.queries
            .retain(|q| !(q.id == id && q.da1_seen && q.result.is_none()));
    }

    /// The write of a colour query failed: nothing will come back.
    pub(crate) fn cancel_colors(&mut self, id: u64) {
        self.queries.retain(|q| q.id != id);
        self.owed
            .retain(|owner| !matches!(owner, Owner::Colors(c) if *c == id));
    }

    /// The completed result of colour query `id`, if it has completed (taken once).
    pub(crate) fn take_colors(&mut self, id: u64) -> Option<TerminalColors> {
        let taken = self
            .queries
            .iter_mut()
            .find(|q| q.id == id)
            .and_then(|q| q.result.take());
        self.retire_colors(id);
        taken
    }

    /// Pi's timer callback (`tui.ts:1484-1487`): the waiter gives up and takes what has arrived so
    /// far; replies from here on are collected for `late`, which fires once the query completes.
    pub(crate) fn time_out_colors(&mut self, id: u64, late: Option<LateColors>) -> TerminalColors {
        let Some(query) = self.queries.iter_mut().find(|q| q.id == id) else {
            return TerminalColors::default();
        };
        if let Some(done) = query.result.take() {
            self.retire_colors(id);
            return done;
        }
        query.timed_out = true;
        query.late = late;
        query.machine.result()
    }

    /// Offer one complete escape sequence. Returns how it was handled and, if it completed a
    /// timed-out query, the late delivery to make after unlocking.
    pub(crate) fn dispatch(&mut self, frame: &str) -> (Dispatch, Option<LateDelivery>) {
        if is_device_attributes_reply(frame) {
            return match self.owed.pop_front() {
                None => (Dispatch::Forward, None),
                Some(Owner::Exchange {
                    abandoned: true, ..
                }) => (Dispatch::Consumed, None),
                Some(Owner::Exchange { ticket, .. }) => (Dispatch::Sentinel(ticket), None),
                Some(Owner::Colors(id)) => {
                    // Pi: the DA1 reply shifts the first pending query and completes it.
                    if let Some(query) = self.queries.iter_mut().find(|q| q.id == id) {
                        query.da1_seen = true;
                    }
                    let delivery = self.finish_colors(id, TerminalColorQuery::complete);
                    self.retire_colors(id);
                    (Dispatch::Consumed, delivery)
                }
            };
        }
        let Some(response) = parse_osc_color_response(frame) else {
            return (Dispatch::Forward, None);
        };
        // Pi hands a colour reply to the FIRST pending query (`pendingTerminalColorQueries[0]`).
        let Some(id) = self.queries.first().map(|q| q.id) else {
            return (Dispatch::Forward, None);
        };
        let delivery = self.finish_colors(id, |m| m.on_osc(response));
        self.retire_colors(id);
        (Dispatch::Consumed, delivery)
    }

    /// Run `step` on query `id`'s machine; if it completed the query, store the result for the
    /// waiter or hand it to the late callback.
    fn finish_colors(
        &mut self,
        id: u64,
        step: impl FnOnce(&mut TerminalColorQuery) -> Option<TerminalColors>,
    ) -> Option<LateDelivery> {
        let query = self.queries.iter_mut().find(|q| q.id == id)?;
        let done = step(&mut query.machine)?;
        if query.timed_out {
            query.late.take().map(|late| (late, done))
        } else {
            query.result = Some(done);
            None
        }
    }
}

/// The process-wide hub: the boot probes write it, and the input reader thread consults it.
static HUB: LazyLock<Mutex<Hub>> = LazyLock::new(|| Mutex::new(Hub::default()));

/// Mirrors `!hub.is_idle()`, so the reader thread can skip the lock for the escape sequences of
/// ordinary typing.
static HUB_BUSY: AtomicBool = AtomicBool::new(false);

fn with_hub<T>(f: impl FnOnce(&mut Hub) -> T) -> T {
    let mut guard = HUB.lock().unwrap_or_else(|e| e.into_inner());
    let out = f(&mut guard);
    HUB_BUSY.store(!guard.is_idle(), Ordering::Release);
    out
}

/// Dispatch one escape sequence on the process-wide hub and make any late delivery.
#[cfg(unix)]
fn dispatch_global(frame: &str) -> Dispatch {
    let (outcome, delivery) = with_hub(|hub| hub.dispatch(frame));
    if let Some((late, colors)) = delivery {
        late(colors);
    }
    outcome
}

/// The input reader's hook: offer a framed escape sequence to the queries still waiting on the
/// terminal. `true` ⇒ it was one of their replies and must not be decoded. Costs one atomic load
/// when no query is in flight.
#[cfg(unix)]
pub(crate) fn offer_reply(sequence: &[u8]) -> bool {
    if !HUB_BUSY.load(Ordering::Acquire) {
        return false;
    }
    let Ok(frame) = std::str::from_utf8(sequence) else {
        return false;
    };
    !matches!(dispatch_global(frame), Dispatch::Forward)
}

// ============================================================================
// Splitting a raw read into complete escape sequences
// ============================================================================

/// Splits the bytes of a synchronous read into complete escape sequences, holding a trailing
/// partial one until the rest arrives. Everything that is not part of an escape sequence — a
/// keystroke typed during startup — is dropped, as it always was: the boot probes only ever looked
/// for replies.
#[derive(Debug, Default)]
struct SequenceSplitter {
    held: Vec<u8>,
}

/// Longest sequence held, so garbage cannot grow the buffer without bound.
const MAX_HELD_SEQUENCE: usize = 4096;

impl SequenceSplitter {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.held.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut start = 0usize;
        loop {
            let Some(esc) = self
                .held
                .get(start..)
                .and_then(|rest| rest.iter().position(|&b| b == 0x1b))
                .map(|offset| start + offset)
            else {
                // No further introducer: the rest is typing.
                start = self.held.len();
                break;
            };
            match sequence_end(self.held.get(esc..).unwrap_or_default()) {
                SequenceEnd::Complete(len) => {
                    if let Some(seq) = self.held.get(esc..esc + len) {
                        out.push(String::from_utf8_lossy(seq).into_owned());
                    }
                    start = esc + len;
                }
                SequenceEnd::Incomplete => {
                    start = esc;
                    break;
                }
            }
        }
        self.held.drain(..start.min(self.held.len()));
        if self.held.len() > MAX_HELD_SEQUENCE {
            self.held.clear();
        }
        out
    }
}

enum SequenceEnd {
    /// The sequence is `len` bytes long, introducer included.
    Complete(usize),
    /// More bytes are needed.
    Incomplete,
}

/// Where the escape sequence at the start of `bytes` (which begins with `ESC`) ends.
fn sequence_end(bytes: &[u8]) -> SequenceEnd {
    let Some(&intro) = bytes.get(1) else {
        return SequenceEnd::Incomplete;
    };
    match intro {
        // CSI: parameter and intermediate bytes, then a final byte 0x40..=0x7e.
        b'[' => match bytes
            .iter()
            .enumerate()
            .skip(2)
            .find(|(_, b)| (0x40..=0x7e).contains(*b))
        {
            Some((index, _)) => SequenceEnd::Complete(index + 1),
            None => SequenceEnd::Incomplete,
        },
        // OSC ends at BEL or ST; DCS, SOS, PM and APC end at ST.
        b']' | b'P' | b'X' | b'^' | b'_' => {
            let bel_ends = intro == b']';
            for (index, &b) in bytes.iter().enumerate().skip(2) {
                if bel_ends && b == 0x07 {
                    return SequenceEnd::Complete(index + 1);
                }
                if b == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                    return SequenceEnd::Complete(index + 2);
                }
            }
            SequenceEnd::Incomplete
        }
        // `ESC` + one character: an Alt chord or an SS3 introducer. Nothing a reply looks like.
        _ => SequenceEnd::Complete(2),
    }
}

// ============================================================================
// The live probe
// ============================================================================

/// What one bounded read of the terminal produced.
///
/// Only the unix reader produces bytes or a timeout; the other platforms' probe reports a closed
/// input, so those two variants are constructed only on unix and in tests.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) enum Read {
    Bytes(Vec<u8>),
    /// Nothing arrived within the bound.
    TimedOut,
    /// End of input or a read error: nothing more will come.
    Closed,
}

/// Where a synchronous query's reply bytes come from — the terminal in production, a script in
/// tests.
pub(crate) trait ReplySource {
    /// Wait at most `timeout` for bytes.
    fn read(&mut self, timeout: Duration) -> Read;
}

/// The production probe: writes the query to stdout and reads the reply straight off stdin under a
/// hard deadline. See the module docs for the timeout / input-safety contract.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdinTerminalProbe;

impl TerminalProbe for StdinTerminalProbe {
    fn query_terminal_colors(
        &self,
        timeout: Duration,
        on_late_reply: Option<LateColors>,
    ) -> TerminalColors {
        query_colors_with(
            tui_stdout(),
            &mut StdinSource,
            timeout,
            on_late_reply,
            stdin_is_queryable,
            &GlobalHub,
        )
    }

    fn query_cell_size(&self, timeout: Duration) -> Option<(u16, u16)> {
        find_cell_size_report(&exchange(tui_stdout(), CELL_SIZE_QUERY, timeout)?)
    }

    fn query_cursor_position(&self, timeout: Duration) -> Option<(u16, u16)> {
        find_cursor_position_report(&exchange(
            crate::dead_terminal::terminal_stdout(),
            CURSOR_POSITION_QUERY,
            timeout,
        )?)
    }
}

/// Both preconditions for a query to be answerable at all: stdin/stdout are a terminal, and raw mode
/// is on (in cooked mode the reply sits in the line discipline until the user presses Enter, which
/// would both hang the probe and hand the escape bytes to the shell).
pub(crate) fn stdin_is_queryable() -> bool {
    use ratatui::crossterm::terminal::is_raw_mode_enabled;
    use ratatui::crossterm::tty::IsTty;
    std::io::stdin().is_tty()
        && std::io::stdout().is_tty()
        && is_raw_mode_enabled().unwrap_or(false)
}

/// The hub a query talks to. Production queries use the process-wide one the reader thread also
/// consults ([`GlobalHub`]); tests hand in a private one.
pub(crate) trait HubAccess {
    /// Run `f` on the hub, then make any late delivery it produced — after the lock is released,
    /// because a callback is free to take it again.
    fn run<T>(&self, f: impl FnOnce(&mut Hub) -> (T, Option<LateDelivery>)) -> T;

    /// [`HubAccess::run`] for an operation that cannot produce a late delivery.
    fn with<T>(&self, f: impl FnOnce(&mut Hub) -> T) -> T {
        self.run(|hub| (f(hub), None))
    }
}

/// The process-wide [`Hub`].
pub(crate) struct GlobalHub;

impl HubAccess for GlobalHub {
    fn run<T>(&self, f: impl FnOnce(&mut Hub) -> (T, Option<LateDelivery>)) -> T {
        let (out, delivery) = with_hub(f);
        if let Some((late, colors)) = delivery {
            late(colors);
        }
        out
    }
}

/// Write [`TERMINAL_COLOR_QUERY`] and wait for the terminal's colours — Pi `queryTerminalColors`
/// (`tui.ts:1470-1493`). Generic over where the bytes come from and which hub tracks the query so
/// the whole exchange can be driven from a scripted terminal.
fn query_colors_with(
    mut out: impl std::io::Write,
    source: &mut dyn ReplySource,
    timeout: Duration,
    on_late_reply: Option<LateColors>,
    queryable: impl FnOnce() -> bool,
    hub: &impl HubAccess,
) -> TerminalColors {
    if !queryable() {
        return TerminalColors::default();
    }
    let id = hub.with(Hub::begin_colors);
    let written = out
        .write_all(TERMINAL_COLOR_QUERY.as_bytes())
        .and_then(|()| out.flush());
    if written.is_err() {
        hub.with(|h| h.cancel_colors(id));
        return TerminalColors::default();
    }

    let deadline = std::time::Instant::now().checked_add(timeout);
    let mut splitter = SequenceSplitter::default();
    loop {
        if let Some(done) = hub.with(|h| h.take_colors(id)) {
            return done;
        }
        let remaining = deadline
            .map(|d| d.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_default();
        if remaining.is_zero() {
            break;
        }
        match source.read(remaining) {
            Read::Bytes(bytes) => {
                for frame in splitter.push(&bytes) {
                    // Whatever is not one of this query's replies is nobody's business here.
                    hub.run(|h| h.dispatch(&frame));
                }
            }
            Read::TimedOut | Read::Closed => break,
        }
    }
    hub.with(|h| h.time_out_colors(id, on_late_reply))
}

/// Write `request` (plus the DA1 sentinel) to `out` and collect whatever comes back within
/// `timeout`.
///
/// `out` is the channel pi writes the same query on. The TUI-level queries — the colours and
/// `CSI 16 t` — go through pi's `this.terminal.write` (`tui.ts:1426`, `:1453`, `:1490`) and are
/// therefore in its write log, so their callers pass [`crate::write_log::tui_stdout`]; the Kitty
/// flags query is `process.stdout.write` (`terminal.ts:261`) and the cursor-position query has no
/// pi counterpart, so those take plain stdout (TUI-040).
///
/// The returned text holds every sequence that was not somebody else's reply, this exchange's own
/// DA1 included — a DA1 owed to an earlier query that gave up is absorbed, not mistaken for this
/// one's. `None` when nothing came back.
///
/// `pub(crate)` so [`mod@crate::keyboard_protocol`]'s Kitty-flags negotiation reuses the SAME bounded,
/// sentinel-terminated exchange rather than opening a second hand-rolled read of stdin — the
/// safety contract in this module's docs is per-read, and one implementation is one contract.
pub(crate) fn exchange(
    out: impl std::io::Write,
    request: &str,
    timeout: Duration,
) -> Option<String> {
    exchange_with(
        out,
        request,
        &mut StdinSource,
        timeout,
        stdin_is_queryable,
        &GlobalHub,
    )
}

pub(crate) fn exchange_with(
    mut out: impl std::io::Write,
    request: &str,
    source: &mut dyn ReplySource,
    timeout: Duration,
    queryable: impl FnOnce() -> bool,
    hub: &impl HubAccess,
) -> Option<String> {
    if !queryable() {
        return None;
    }
    let ticket = hub.with(Hub::begin_exchange);
    let written = out
        .write_all(request.as_bytes())
        .and_then(|()| out.write_all(DEVICE_ATTRIBUTES_QUERY.as_bytes()))
        .and_then(|()| out.flush());
    if written.is_err() {
        hub.with(|h| h.abandon_exchange(ticket));
        return None;
    }

    let deadline = std::time::Instant::now().checked_add(timeout)?;
    let mut splitter = SequenceSplitter::default();
    let mut forwarded = String::new();
    let mut total = 0usize;
    let mut sentinel = false;
    'read: while !sentinel {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match source.read(remaining) {
            Read::Bytes(bytes) => {
                total = total.saturating_add(bytes.len());
                for frame in splitter.push(&bytes) {
                    match hub.run(|h| h.dispatch(&frame)) {
                        Dispatch::Consumed => {}
                        Dispatch::Forward => forwarded.push_str(&frame),
                        Dispatch::Sentinel(t) if t == ticket => {
                            forwarded.push_str(&frame);
                            sentinel = true;
                            break;
                        }
                        // A DA1 owed to a different live exchange cannot happen on one thread.
                        Dispatch::Sentinel(_) => {}
                    }
                }
                if total >= max_reply_bytes() {
                    break 'read;
                }
            }
            Read::TimedOut | Read::Closed => break,
        }
    }
    if !sentinel {
        hub.with(|h| h.abandon_exchange(ticket));
    }
    (!forwarded.is_empty()).then_some(forwarded)
}

/// Write the colour query and return at once; `on_colors` receives the finished result whenever
/// the terminal completes it. This is Pi's `queryTerminalColors` without awaiting it, for the
/// moments the input reader thread already owns stdin: the reader offers every reply to the hub
/// ([`offer_reply`]), which absorbs this query's replies and calls `on_colors`.
///
/// A no-op unless stdin and stdout are a terminal in raw mode.
#[cfg(unix)]
pub(crate) fn request_terminal_colors_async(out: impl std::io::Write, on_colors: LateColors) {
    request_terminal_colors_async_with(out, on_colors, stdin_is_queryable, &GlobalHub);
}

#[cfg(unix)]
pub(crate) fn request_terminal_colors_async_with(
    mut out: impl std::io::Write,
    on_colors: LateColors,
    queryable: impl FnOnce() -> bool,
    hub: &impl HubAccess,
) {
    if !queryable() {
        return;
    }
    let id = hub.with(|h| {
        let id = h.begin_colors();
        // Nobody waits on this query: it is born timed out, and completion is delivered.
        h.time_out_colors(id, Some(on_colors));
        id
    });
    let written = out
        .write_all(TERMINAL_COLOR_QUERY.as_bytes())
        .and_then(|()| out.flush());
    if written.is_err() {
        hub.with(|h| h.cancel_colors(id));
    }
}

#[cfg(unix)]
const fn max_reply_bytes() -> usize {
    MAX_REPLY_BYTES
}

#[cfg(not(unix))]
const fn max_reply_bytes() -> usize {
    usize::MAX
}

/// Stdin, read under a `poll(2)` bound. Never blocks past the timeout and never consumes a byte the
/// terminal did not send in reply.
pub(crate) struct StdinSource;

#[cfg(unix)]
impl ReplySource for StdinSource {
    fn read(&mut self, timeout: Duration) -> Read {
        use rustix::event::{PollFd, PollFlags, Timespec};
        use rustix::io::Errno;

        let stdin = std::io::stdin();
        let ts = Timespec {
            tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX) as _,
            tv_nsec: i64::from(timeout.subsec_nanos()) as _,
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&ts)) {
            // Timed out: the terminal does not answer this query. Nothing was consumed.
            Ok(0) => return Read::TimedOut,
            Ok(_) => {}
            // A signal interrupted the wait; the caller re-checks its deadline and asks again.
            Err(Errno::INTR) => return Read::Bytes(Vec::new()),
            Err(_) => return Read::Closed,
        }
        let mut chunk = [0u8; 128];
        match rustix::io::read(&stdin, &mut chunk[..]) {
            Ok(0) => Read::Closed,
            Ok(n) => Read::Bytes(chunk.get(..n).unwrap_or_default().to_vec()),
            Err(Errno::INTR | Errno::AGAIN) => Read::Bytes(Vec::new()),
            Err(_) => Read::Closed,
        }
    }
}

/// Non-Unix has no `poll`-able stdin fd here; detection falls back to `COLORFGBG`.
#[cfg(not(unix))]
impl ReplySource for StdinSource {
    fn read(&mut self, _timeout: Duration) -> Read {
        Read::Closed
    }
}

#[cfg(test)]
#[path = "terminal_query_tests.rs"]
mod tests;
