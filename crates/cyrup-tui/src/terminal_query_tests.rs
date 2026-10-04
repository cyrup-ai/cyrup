#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A terminal that answers from a script, one chunk per read, and then falls silent. Silence is a
/// `TimedOut` the instant the script runs dry, so a "terminal answers nothing" test costs nothing.
struct Script {
    chunks: VecDeque<Vec<u8>>,
    /// Run once before the first read — the moment a second query is written on the same terminal.
    on_first_read: Option<Box<dyn FnOnce()>>,
}

impl Script {
    fn new(chunks: &[&str]) -> Self {
        Self {
            chunks: chunks.iter().map(|c| c.as_bytes().to_vec()).collect(),
            on_first_read: None,
        }
    }
}

impl ReplySource for Script {
    fn read(&mut self, _timeout: Duration) -> Read {
        if let Some(hook) = self.on_first_read.take() {
            hook();
        }
        match self.chunks.pop_front() {
            Some(bytes) => Read::Bytes(bytes),
            None => Read::TimedOut,
        }
    }
}

impl HubAccess for Mutex<Hub> {
    fn run<T>(&self, f: impl FnOnce(&mut Hub) -> (T, Option<LateDelivery>)) -> T {
        let (out, delivery) = f(&mut self.lock().unwrap());
        if let Some((late, colors)) = delivery {
            late(colors);
        }
        out
    }
}

const TIMEOUT: Duration = Duration::from_secs(5);
const DA1: &str = "\x1b[?62;1;2;6;9;15;22c";

fn osc10(rgb: &str) -> String {
    format!("\x1b]10;{rgb}\x07")
}
fn osc11(rgb: &str) -> String {
    format!("\x1b]11;{rgb}\x07")
}
fn osc4(index: usize, rgb: &str) -> String {
    format!("\x1b]4;{index};{rgb}\x07")
}

/// Slot `i` reports `rgb:(i*16)/(i*16)/(i*16)` as xterm does — 16-bit channels.
fn palette_reply(index: usize) -> String {
    let v = index * 16;
    osc4(
        index,
        &format!("rgb:{v:02x}{v:02x}/{v:02x}{v:02x}/{v:02x}{v:02x}"),
    )
}

fn expected_palette() -> [Rgb; 16] {
    std::array::from_fn(|i| {
        let v = u8::try_from(i * 16).unwrap();
        Rgb::new(v, v, v)
    })
}

fn colors_with(script: &mut Script, hub: &Mutex<Hub>, late: Option<LateColors>) -> TerminalColors {
    query_colors_with(std::io::sink(), script, TIMEOUT, late, || true, hub)
}

// ------------------------------------------------------------------ the batch itself ----

/// `TERMINAL_COLOR_QUERY` (`tui.ts:168-172` @v1.0.0): OSC 10, OSC 11, sixteen OSC 4, one DA1.
#[test]
fn the_colour_query_is_pis_batch() {
    let mut expected = String::from("\x1b]10;?\x07\x1b]11;?\x07");
    for i in 0..16 {
        expected.push_str(&format!("\x1b]4;{i};?\x07"));
    }
    expected.push_str("\x1b[c");
    assert_eq!(TERMINAL_COLOR_QUERY.as_str(), expected);
    assert_eq!(TERMINAL_COLOR_QUERY.matches("?\x07").count(), 18);
    assert!(
        TERMINAL_COLOR_QUERY.ends_with("\x1b[c"),
        "DA1 closes the batch"
    );
}

/// Every expected value below was produced by running pi's `parseOscColorResponse`
/// (`terminal-colors.ts` @v1.0.0, Node 22) over the same string.
#[test]
fn osc_colour_replies_parse_as_pi_parses_them() {
    use OscColorTarget::{Background, Foreground, Palette};
    let ok = |target, r, g, b| {
        Some(OscColorResponse {
            target,
            rgb: Some(Rgb::new(r, g, b)),
        })
    };
    let unparseable = |target| Some(OscColorResponse { target, rgb: None });
    for (input, expected) in [
        ("\x1b]10;#e5e5e7\x07", ok(Foreground, 229, 229, 231)),
        ("\x1b]11;#1e1e1e\x07", ok(Background, 30, 30, 30)),
        // `ST` terminates as well as `BEL`; 12-digit hex has 16-bit channels.
        ("\x1b]11;#ffffffffffff\x1b\\", ok(Background, 255, 255, 255)),
        ("\x1b]11;#FFFFFF\x07", ok(Background, 255, 255, 255)),
        // xterm's half intensity `8080` rounds to 128, matching `Math.round`.
        (
            "\x1b]11;rgb:8080/8080/8080\x07",
            ok(Background, 128, 128, 128),
        ),
        ("\x1b]11;rgb:00/00/00\x07", ok(Background, 0, 0, 0)),
        // `rgba:` is accepted case-insensitively, and the alpha is ignored — as is a fourth
        // component after `rgb:`, because pi destructures the first three.
        ("\x1b]11;RGBA:ff/00/00\x07", ok(Background, 255, 0, 0)),
        ("\x1b]11;rgba:ff/00/00/ff\x07", ok(Background, 255, 0, 0)),
        ("\x1b]11;rgb:ff/00/00/ff\x07", ok(Background, 255, 0, 0)),
        // Channels of any width scale by their own maximum.
        ("\x1b]11;rgb:f/0/8\x07", ok(Background, 255, 0, 136)),
        (
            "\x1b]11;rgb:fffff/00000/80000\x07",
            ok(Background, 255, 0, 128),
        ),
        ("\x1b]11; rgb:ff/00/00 \x07", ok(Background, 255, 0, 0)),
        // OSC 4: one to three digits of palette index, leading zeros included.
        ("\x1b]4;0;rgb:0000/0000/0000\x07", ok(Palette(0), 0, 0, 0)),
        (
            "\x1b]4;15;rgb:ffff/ffff/ffff\x07",
            ok(Palette(15), 255, 255, 255),
        ),
        ("\x1b]4;255;#abcdef\x07", ok(Palette(255), 171, 205, 239)),
        ("\x1b]4;999;#abcdef\x07", ok(Palette(999), 171, 205, 239)),
        ("\x1b]4;03;#000000\x07", ok(Palette(3), 0, 0, 0)),
        ("\x1b]4;3;#000000\x1b\\", ok(Palette(3), 0, 0, 0)),
        // A reply whose colour does not parse is still a reply.
        ("\x1b]4;7;#zzzzzz\x07", unparseable(Palette(7))),
        ("\x1b]4;7;#12345\x07", unparseable(Palette(7))),
        ("\x1b]4;7;\x07", unparseable(Palette(7))),
        ("\x1b]11;rgb:ff/00\x07", unparseable(Background)),
        ("\x1b]11;rgb:gg/00/00\x07", unparseable(Background)),
        // Not a colour reply at all.
        ("\x1b]4;1000;#abcdef\x07", None),
        ("\x1b]4;;#abcdef\x07", None),
        ("\x1b]12;#ffffff\x07", None),
        ("\x1b]10;#ffffff", None),
        ("\x1b]10;#ff\x07ff\x07", None),
        ("\x1b]11;#1e1e1e\x07x", None),
        ("x\x1b]11;#1e1e1e\x07", None),
    ] {
        assert_eq!(parse_osc_color_response(input), expected, "{input:?}");
    }
}

// ---------------------------------------------- a fake terminal answering the whole batch ----

/// The row's first verify item: all eighteen colours, then DA1, give a full `TerminalColors`.
#[test]
fn a_terminal_answering_all_eighteen_colours_then_da1_yields_the_full_colours() {
    let hub = Mutex::new(Hub::default());
    let mut chunks = vec![osc10("#d0d0d0"), osc11("#101010")];
    chunks.extend((0..16).map(palette_reply));
    chunks.push(DA1.to_string());
    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();

    let mut script = Script::new(&refs);
    let colors = colors_with(&mut script, &hub, None);

    assert_eq!(colors.foreground, Some(Rgb::new(0xd0, 0xd0, 0xd0)));
    assert_eq!(colors.background, Some(Rgb::new(0x10, 0x10, 0x10)));
    assert_eq!(colors.palette, Some(expected_palette()));
    // The eighteenth reply completed the query, so the DA1 was never read by it; the ledger still
    // owes that reply and absorbs it when the reader thread delivers it.
    assert_eq!(script.chunks.len(), 1, "the DA1 chunk was left unread");
    let (outcome, _) = hub.lock().unwrap().dispatch(DA1);
    assert_eq!(outcome, Dispatch::Consumed);
    assert!(
        hub.lock().unwrap().is_idle(),
        "the DA1 reply retired the ledger entry"
    );
}

/// Completion on the DA1 reply alone: a terminal that answers the OSC queries partially still ends
/// the wait at the sentinel, and the foreground and background survive without a palette.
#[test]
fn fifteen_palette_slots_yield_no_palette_but_keep_the_foreground_and_background() {
    let hub = Mutex::new(Hub::default());
    let mut chunks = vec![osc10("#d0d0d0"), osc11("#101010")];
    chunks.extend((0..15).map(palette_reply));
    chunks.push(DA1.to_string());
    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();

    let colors = colors_with(&mut Script::new(&refs), &hub, None);

    assert_eq!(colors.foreground, Some(Rgb::new(0xd0, 0xd0, 0xd0)));
    assert_eq!(colors.background, Some(Rgb::new(0x10, 0x10, 0x10)));
    assert_eq!(colors.palette, None, "all sixteen or nothing");
}

/// A terminal that ignores every OSC but answers DA1 settles immediately with nothing — pi's
/// "including for terminals that ignore the colour queries".
#[test]
fn a_terminal_that_answers_only_da1_settles_with_nothing_reported() {
    let hub = Mutex::new(Hub::default());
    let colors = colors_with(&mut Script::new(&[DA1]), &hub, None);
    assert_eq!(colors, TerminalColors::default());
}

/// "One that answers nothing settles at the timeout with everything `None`."
#[test]
fn a_terminal_that_answers_nothing_settles_at_the_timeout_with_everything_none() {
    let hub = Mutex::new(Hub::default());
    let started = std::time::Instant::now();
    let colors = query_colors_with(
        std::io::sink(),
        &mut Script::new(&[]),
        Duration::from_millis(30),
        None,
        || true,
        &hub,
    );
    assert_eq!(colors, TerminalColors::default());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        !hub.lock().unwrap().is_idle(),
        "the query stays listed: its DA1 is still owed and a late reply may still land"
    );
}

/// Eighteen distinct replies complete the query without waiting for DA1; the DA1 that follows is
/// absorbed by the query that asked, not forwarded.
#[test]
fn eighteen_replies_complete_the_query_before_da1_and_the_da1_is_absorbed_later() {
    let hub = Mutex::new(Hub::default());
    let mut chunks = vec![osc10("#d0d0d0"), osc11("#101010")];
    chunks.extend((0..16).map(palette_reply));
    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();

    let colors = colors_with(&mut Script::new(&refs), &hub, None);
    assert_eq!(colors.palette, Some(expected_palette()));

    // The terminal's DA1 arrives afterwards — in production at the input reader.
    let (outcome, _) = hub.lock().unwrap().dispatch(DA1);
    assert_eq!(outcome, Dispatch::Consumed);
    assert!(hub.lock().unwrap().is_idle());
}

/// A duplicate reply must not count twice (`replied: Set`, `tui.ts:151`): seventeen distinct
/// targets plus a repeat is not eighteen, so the query keeps waiting for its DA1.
#[test]
fn a_duplicate_reply_does_not_count_twice() {
    let hub = Mutex::new(Hub::default());
    let mut chunks = vec![osc10("#d0d0d0"), osc11("#101010")];
    chunks.extend((0..15).map(palette_reply));
    chunks.push(palette_reply(3));
    chunks.push(palette_reply(14));
    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
    let mut script = Script::new(&refs);

    let colors = colors_with(&mut script, &hub, None);

    // The script ran dry before DA1 — i.e. the query did NOT complete on the repeats.
    assert!(script.chunks.is_empty());
    assert_eq!(colors.palette, None, "slot 15 never answered");
    assert_eq!(colors.foreground, Some(Rgb::new(0xd0, 0xd0, 0xd0)));
}

/// A repeat of a target keeps the FIRST colour it reported.
#[test]
fn a_repeated_target_keeps_its_first_colour() {
    let hub = Mutex::new(Hub::default());
    let chunks = [osc11("#101010"), osc11("#ffffff"), DA1.to_string()];
    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
    let colors = colors_with(&mut Script::new(&refs), &hub, None);
    assert_eq!(colors.background, Some(Rgb::new(0x10, 0x10, 0x10)));
}

// --------------------------------------------------------------- late replies ----

/// "A slot answered AFTER the timeout reaches the late-reply callback."
#[test]
fn a_slot_answered_after_the_timeout_reaches_the_late_reply_callback() {
    let hub = Mutex::new(Hub::default());
    let late_calls = Arc::new(Mutex::new(Vec::<TerminalColors>::new()));
    let sink = {
        let late_calls = Arc::clone(&late_calls);
        Arc::new(move |colors: TerminalColors| late_calls.lock().unwrap().push(colors))
            as LateColors
    };

    // Everything but the last six palette slots arrives in time; then the terminal stalls.
    let mut chunks = vec![osc10("#d0d0d0"), osc11("#101010")];
    chunks.extend((0..10).map(palette_reply));
    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
    let on_time = colors_with(&mut Script::new(&refs), &hub, Some(sink));

    assert_eq!(
        on_time.palette, None,
        "ten of sixteen slots is not a palette"
    );
    assert_eq!(on_time.background, Some(Rgb::new(0x10, 0x10, 0x10)));
    assert!(late_calls.lock().unwrap().is_empty(), "nothing late yet");

    // The stragglers land at the input reader: still no completion, so still no callback.
    for slot in 10..15 {
        let (outcome, delivery) = hub.lock().unwrap().dispatch(&palette_reply(slot));
        assert_eq!(
            outcome,
            Dispatch::Consumed,
            "slot {slot} belongs to the query"
        );
        assert!(delivery.is_none());
    }
    // The sixteenth slot completes the batch (eighteen distinct replies).
    let (outcome, delivery) = hub.lock().unwrap().dispatch(&palette_reply(15));
    assert_eq!(outcome, Dispatch::Consumed);
    let (late, colors) = delivery.expect("the eighteenth reply completes the timed-out query");
    late(colors);

    let calls = late_calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "delivered exactly once");
    let call = calls.first().unwrap();
    assert_eq!(call.palette, Some(expected_palette()));
    assert_eq!(call.foreground, Some(Rgb::new(0xd0, 0xd0, 0xd0)));
    assert_eq!(call.background, Some(Rgb::new(0x10, 0x10, 0x10)));

    // The DA1 that follows is still the query's, and delivers nothing a second time.
    let (outcome, delivery) = hub.lock().unwrap().dispatch(DA1);
    assert_eq!(outcome, Dispatch::Consumed);
    assert!(delivery.is_none());
    assert!(hub.lock().unwrap().is_idle());
}

/// The late path also completes on DA1 alone — a terminal over a slow link whose DA1 trails its
/// colours past the deadline.
#[test]
fn a_late_da1_completes_a_timed_out_query_with_what_arrived() {
    let hub = Mutex::new(Hub::default());
    let count = Arc::new(AtomicUsize::new(0));
    let last = Arc::new(Mutex::new(None));
    let sink = {
        let (count, last) = (Arc::clone(&count), Arc::clone(&last));
        Arc::new(move |colors: TerminalColors| {
            count.fetch_add(1, Ordering::SeqCst);
            *last.lock().unwrap() = Some(colors);
        }) as LateColors
    };

    let on_time = colors_with(&mut Script::new(&[]), &hub, Some(sink));
    assert_eq!(on_time, TerminalColors::default());

    let (_, none) = hub.lock().unwrap().dispatch(&osc11("#fafafa"));
    assert!(none.is_none());
    let (outcome, delivery) = hub.lock().unwrap().dispatch(DA1);
    assert_eq!(outcome, Dispatch::Consumed);
    let (late, colors) = delivery.unwrap();
    late(colors);

    assert_eq!(count.load(Ordering::SeqCst), 1);
    let delivered = last.lock().unwrap().unwrap();
    assert_eq!(delivered.background, Some(Rgb::new(0xfa, 0xfa, 0xfa)));
    assert_eq!(delivered.palette, None);
}

/// With no callback a late result is dropped, but the DA1 is still accounted for.
#[test]
fn a_late_reply_with_no_callback_is_absorbed_silently() {
    let hub = Mutex::new(Hub::default());
    colors_with(&mut Script::new(&[]), &hub, None);
    let (outcome, delivery) = hub.lock().unwrap().dispatch(&osc11("#fafafa"));
    assert_eq!(outcome, Dispatch::Consumed);
    assert!(delivery.is_none());
    let (outcome, delivery) = hub.lock().unwrap().dispatch(DA1);
    assert_eq!(outcome, Dispatch::Consumed);
    assert!(delivery.is_none());
    assert!(hub.lock().unwrap().is_idle());
}

// -------------------------------------------------- who owns which DA1 reply ----

/// The row's last verify item, at the ledger: "With the keyboard-protocol query in flight, the
/// first DA1 reply ends negotiation and the second reaches the colour query."
#[test]
fn the_first_da1_ends_the_negotiation_and_the_second_reaches_the_colour_query() {
    let mut hub = Hub::default();
    let negotiation = hub.begin_exchange();
    let colours = hub.begin_colors();

    // Terminals answer in order: the Kitty flags, DA1 #1, the colours, DA1 #2.
    let (flags, _) = hub.dispatch("\x1b[?1u");
    assert_eq!(
        flags,
        Dispatch::Forward,
        "the flags reply is the negotiator's to read"
    );
    let (first_da1, _) = hub.dispatch(DA1);
    assert_eq!(first_da1, Dispatch::Sentinel(negotiation));

    let (osc, _) = hub.dispatch(&osc11("#101010"));
    assert_eq!(osc, Dispatch::Consumed);
    assert!(
        hub.take_colors(colours).is_none(),
        "not complete before its DA1"
    );
    let (second_da1, _) = hub.dispatch(DA1);
    assert_eq!(second_da1, Dispatch::Consumed);
    let done = hub
        .take_colors(colours)
        .expect("the second DA1 completed the colour query");
    assert_eq!(done.background, Some(Rgb::new(0x10, 0x10, 0x10)));
    assert!(hub.is_idle());
}

/// An exchange that gave up keeps its place: its late DA1 is absorbed, so the NEXT query does not
/// take it for its own sentinel and finish early — the failure a bare "first DA1 wins" read has.
#[test]
fn a_timed_out_exchanges_late_da1_is_not_taken_for_the_next_querys() {
    let hub = Mutex::new(Hub::default());
    // Exchange A: the terminal is slow and the script is dry, so A gives up.
    let a = exchange_with(
        std::io::sink(),
        "\x1b[?u",
        &mut Script::new(&[]),
        TIMEOUT,
        || true,
        &hub,
    );
    assert_eq!(a, None);

    // Exchange B: A's DA1 finally lands first, then B's real answer and B's own DA1.
    let b = exchange_with(
        std::io::sink(),
        CELL_SIZE_QUERY,
        &mut Script::new(&[DA1, "\x1b[6;18;9t", DA1]),
        TIMEOUT,
        || true,
        &hub,
    )
    .expect("B got its reply");
    assert_eq!(find_cell_size_report(&b), Some((9, 18)));
    assert!(hub.lock().unwrap().is_idle());
}

/// The same hazard from the colour query's side: a slow negotiation's DA1 must not complete it.
#[test]
fn a_timed_out_exchanges_late_da1_does_not_complete_a_colour_query() {
    let hub = Mutex::new(Hub::default());
    exchange_with(
        std::io::sink(),
        "\x1b[?u",
        &mut Script::new(&[]),
        TIMEOUT,
        || true,
        &hub,
    );
    let chunks = [DA1.to_string(), osc11("#101010"), DA1.to_string()];
    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
    let colors = colors_with(&mut Script::new(&refs), &hub, None);
    assert_eq!(
        colors.background,
        Some(Rgb::new(0x10, 0x10, 0x10)),
        "the first DA1 was the negotiation's"
    );
}

/// An exchange hands its caller the replies that were not somebody else's, own DA1 included.
#[test]
fn an_exchange_returns_its_replies_and_its_own_sentinel() {
    let hub = Mutex::new(Hub::default());
    let got = exchange_with(
        std::io::sink(),
        CURSOR_POSITION_QUERY,
        &mut Script::new(&["\x1b[12;40R", DA1]),
        TIMEOUT,
        || true,
        &hub,
    )
    .unwrap();
    assert_eq!(find_cursor_position_report(&got), Some((39, 11)));
    assert!(saw_device_attributes(got.as_bytes()));
}

/// A dead terminal is skipped, not awaited.
#[test]
fn an_unqueryable_terminal_is_never_written_to_or_waited_on() {
    let hub = Mutex::new(Hub::default());
    let mut sink = Vec::new();
    let got = exchange_with(
        &mut sink,
        "\x1b[?u",
        &mut Script::new(&[DA1]),
        TIMEOUT,
        || false,
        &hub,
    );
    assert_eq!(got, None);
    assert!(
        sink.is_empty(),
        "nothing is written to a terminal that cannot answer"
    );
    assert!(hub.lock().unwrap().is_idle());

    let colors = query_colors_with(
        &mut sink,
        &mut Script::new(&[DA1]),
        TIMEOUT,
        None,
        || false,
        &hub,
    );
    assert_eq!(colors, TerminalColors::default());
    assert!(sink.is_empty());
}

// ------------------------------------------------------------ the sequence splitter ----

#[test]
fn the_splitter_extracts_complete_sequences_and_holds_a_partial_one() {
    let mut s = SequenceSplitter::default();
    assert_eq!(
        s.push(b"ab\x1b]11;rgb:00/00/00\x07\x1b[?62;c"),
        vec![
            "\x1b]11;rgb:00/00/00\x07".to_string(),
            "\x1b[?62;c".to_string()
        ],
        "typing between sequences is dropped"
    );
    // A reply split across reads comes out whole once complete.
    assert!(s.push(b"\x1b]4;7;rgb:ff").is_empty());
    assert_eq!(
        s.push(b"/ff/ff\x1b\\"),
        vec!["\x1b]4;7;rgb:ff/ff/ff\x1b\\".to_string()]
    );
    assert!(s.push(b"\x1b[?6").is_empty());
    assert_eq!(s.push(b"2c"), vec!["\x1b[?62c".to_string()]);
}

// ----------------------------------------------- unchanged probes (cell, cursor, scheme) ----

#[test]
fn cell_size_report_forms() {
    // Pi's `/^\x1b\[6;(\d+);(\d+)t$/` is HEIGHT then WIDTH; the tuple here is (width, height),
    // the order `setCellDimensions({widthPx, heightPx})` restores.
    assert_eq!(parse_cell_size_report("\x1b[6;18;9t"), Some((9, 18)));
    assert_eq!(parse_cell_size_report("\x1b[6;40;20t"), Some((20, 40)));
    // Pi's `heightPx <= 0 || widthPx <= 0` guard (`tui.ts:885`).
    assert_eq!(parse_cell_size_report("\x1b[6;0;9t"), None);
    assert_eq!(parse_cell_size_report("\x1b[6;18;0t"), None);
    // Shape rejections: not a `6` report, a missing field, a non-numeric field, no terminator.
    assert_eq!(
        parse_cell_size_report("\x1b[4;18;9t"),
        None,
        "CSI 4 t is the pixel SIZE report"
    );
    assert_eq!(parse_cell_size_report("\x1b[6;18t"), None);
    assert_eq!(parse_cell_size_report("\x1b[6;18;xt"), None);
    assert_eq!(parse_cell_size_report("\x1b[6;18;9"), None);
    assert_eq!(
        parse_cell_size_report("\x1b[16t"),
        None,
        "the QUERY is not a report"
    );
}

#[test]
fn cell_size_is_found_alongside_the_sentinel_answer() {
    // What a Kitty-class terminal sends back for `CSI 16 t` + `CSI c`, in one read.
    assert_eq!(
        find_cell_size_report("\x1b[6;18;9t\x1b[?62;1;2c"),
        Some((9, 18))
    );
    assert_eq!(
        find_cell_size_report("\x1b[?62;1;2c"),
        None,
        "DA1 only ⇒ no cell size"
    );
    assert_eq!(find_cell_size_report(""), None);
}

#[test]
fn cursor_position_report_forms() {
    // `CSI <row> ; <col> R` is ROW then COLUMN, 1-based; the tuple here is 0-based `(col, row)`
    // — the order `ratatui::layout::Position { x, y }` wants.
    assert_eq!(parse_cursor_position_report("\x1b[12;40R"), Some((39, 11)));
    assert_eq!(
        parse_cursor_position_report("\x1b[1;1R"),
        Some((0, 0)),
        "top-left is (0, 0)"
    );
    // DECXCPR's leading `?` is tolerated even though cyrup never sends the DECXCPR query.
    assert_eq!(parse_cursor_position_report("\x1b[?12;40R"), Some((39, 11)));
    // CPR coordinates are 1-based; a `0` is rejected rather than underflowed.
    assert_eq!(
        parse_cursor_position_report("\x1b[0;5R"),
        None,
        "row 0 cannot underflow"
    );
    assert_eq!(
        parse_cursor_position_report("\x1b[5;0R"),
        None,
        "col 0 cannot underflow"
    );
    // Shape rejections: no terminator, missing field, non-numeric field, wrong introducer.
    assert_eq!(
        parse_cursor_position_report("\x1b[12;40"),
        None,
        "missing R terminator"
    );
    assert_eq!(
        parse_cursor_position_report("\x1b[12R"),
        None,
        "missing `;`"
    );
    assert_eq!(
        parse_cursor_position_report("\x1b[;40R"),
        None,
        "empty row field"
    );
    assert_eq!(
        parse_cursor_position_report("\x1b[12;R"),
        None,
        "empty col field"
    );
    assert_eq!(
        parse_cursor_position_report("\x1b[a;40R"),
        None,
        "non-digit row field"
    );
    assert_eq!(
        parse_cursor_position_report("12;40R"),
        None,
        "missing CSI introducer"
    );
    assert_eq!(
        parse_cursor_position_report("\x1b[6n"),
        None,
        "the QUERY is not a report"
    );
}

#[test]
fn cursor_position_is_found_alongside_the_sentinel_answer() {
    // What a real xterm sends back for `CSI 6 n` + `CSI c`, in one read — the exact wire shape
    // `exchange(_, CURSOR_POSITION_QUERY, ..)` produces.
    assert_eq!(
        find_cursor_position_report("\x1b[12;40R\x1b[?62;1;2c"),
        Some((39, 11))
    );
    assert_eq!(
        find_cursor_position_report("\x1b[?62;1;2c"),
        None,
        "DA1 only ⇒ no position"
    );
    assert_eq!(find_cursor_position_report(""), None);
}

#[test]
fn color_scheme_report_forms() {
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;1n"),
        Some(TerminalTheme::Dark)
    );
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;2n"),
        Some(TerminalTheme::Light)
    );
    assert_eq!(parse_color_scheme_report("\x1b[?997;3n"), None);
    assert_eq!(
        parse_color_scheme_report("\x1b[?996n"),
        None,
        "the QUERY is not a report"
    );
}

/// Pi v0.84.1 `tui/test/terminal-colors.test.ts:118-122`, transcribed case for case.
#[test]
fn a_batched_color_scheme_burst_settles_on_the_last_report() {
    // `terminal-colors.test.ts:118` — "\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n" ⇒ "dark".
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n"),
        Some(TerminalTheme::Dark),
    );
    // `terminal-colors.test.ts:119` — "\x1b[?997;1n\x1b[?997;2n\x1b[?997;2n" ⇒ "light".
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;1n\x1b[?997;2n\x1b[?997;2n"),
        Some(TerminalTheme::Light),
    );
    // Two frames is enough to tell first-wins from last-wins.
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;1n\x1b[?997;2n"),
        Some(TerminalTheme::Light),
    );
    // MIRROR: Pi stays anchored, so a burst is all-or-nothing. `terminal-colors.test.ts:120-122`.
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;2n\x1b[?997;3n"),
        None,
        "one malformed frame poisons the whole burst",
    );
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;2n\x1b[?997;"),
        None,
        "a truncated trailing frame poisons the whole burst",
    );
    assert_eq!(
        parse_color_scheme_report("x\x1b[?997;1n"),
        None,
        "test.ts:122 — leading junk"
    );
    assert_eq!(
        parse_color_scheme_report("\x1b[?997;1nx"),
        None,
        "trailing junk"
    );
    assert_eq!(
        parse_color_scheme_report(""),
        None,
        "`+` demands at least one frame"
    );
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
    assert_eq!(
        StdinTerminalProbe.query_terminal_colors(Duration::from_secs(30), None),
        TerminalColors::default()
    );
    assert_eq!(
        StdinTerminalProbe.query_cell_size(Duration::from_secs(30)),
        None
    );
    assert_eq!(
        StdinTerminalProbe.query_cursor_position(Duration::from_secs(30)),
        None
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the probe must not block"
    );
}

/// The trait defaults answer "nothing", regardless of timeout.
#[test]
fn the_default_probes_answer_nothing() {
    assert_eq!(
        NoTerminalProbe.query_terminal_colors(Duration::from_secs(30), None),
        TerminalColors::default()
    );
    assert_eq!(
        NoTerminalProbe.query_cell_size(Duration::from_secs(30)),
        None
    );
    assert_eq!(
        NoTerminalProbe.query_cursor_position(Duration::from_secs(30)),
        None
    );
}
