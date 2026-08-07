//! Swallow a **late** terminal OSC 11 reply before it reaches the editor as keystrokes.
//!
//! This is the port of Pi's first line of defence in `handleTerminalInput`
//! (`pi/packages/tui/src/tui.ts:788-794`), which begins every dispatch with
//! `consumeOsc11BackgroundResponse(data)` / `consumeTerminalColorSchemeReport(data)` and returns
//! early, so a background-colour answer arriving at *any* time — including long after the probe
//! that asked for it gave up — is discarded instead of being handed to an input listener.
//!
//! [`crate::terminal_query`] already reads the reply under a hard deadline, and its own module doc
//! names the residual risk: a terminal that answers *after* that deadline. When that happens the
//! probe is gone, [`crate::app::crossterm_input_stream`]'s reader thread is running, and crossterm
//! decodes the reply as ordinary keys. The user sees `11;rgb:0c0c/0b0b/1313` typed into the prompt.
//!
//! # Why this cannot be a byte-level filter like Pi's
//!
//! Pi filters **raw bytes** before dispatch. By the time cyrup sees input, crossterm has already
//! parsed it. `parse_event` (`crossterm-0.29.0/src/event/sys/unix/parse.rs:26-125`) routes
//! `ESC`-followed-by-anything-other-than-`O`/`[`/`ESC` to its Alt-key fallback arm (`:77-88`), so
//! the OSC introducer is destroyed and the frame surfaces as this exact event sequence:
//!
//! ```text
//! ESC ] 1 1 ; r g b : 0 c 0 c / 0 b 0 b / 1 3 1 3 BEL
//!  └──┬──┘  └──────────────── payload ───────────┘ └─ terminator
//! Key(']', ALT)  Key('1') Key('1') Key(';')  …  Key('g', CONTROL)
//! ```
//!
//! `ESC` + `]` are collapsed into a **single** `Alt+]` press (there is no separate `Esc` event),
//! and `BEL` (0x07) lands in crossterm's C0 arm (`parse.rs:105-108`, `c - 0x1 + b'a'`) so it is
//! indistinguishable from `Ctrl+G`. When the reply is split across `read(2)` calls exactly at the
//! `ESC` byte the opener instead arrives as `Key(Esc)` then `Key(']')` with no modifier, so both
//! opener forms are recognised. An `ST` terminator (`ESC \`) likewise arrives as `Alt+\` or as
//! `Esc` then `\`.
//!
//! # The safety contract (this filter sits on the path every keystroke takes)
//!
//! Eating one real keystroke would be far worse than the leak being fixed, so the machine is built
//! so that **the only thing it can ever remove is a complete, correctly terminated OSC 11 frame**:
//!
//! 1. The opener is *ambiguous* and never commits. A genuine `Alt+]` press produces a byte-identical
//!    event — under the Kitty `DISAMBIGUATE_ESCAPE_CODES` flags cyrup pushes
//!    (`app.rs`, `App::into_stdout`), `ESC [ 93 ; 3 u` also decodes to `Char(']') + ALT`. So the
//!    opener only *holds*.
//! 2. Held events are **replayed in order** the instant anything fails to match: a wrong tail, a
//!    payload character outside the OSC 11 alphabet, a non-key event, the [`MAX_HELD`] cap, or the
//!    input simply going idle ([`StrayReplyFilter::flush`], driven by the reader thread's poll
//!    timeout). Nothing is dropped on a "give up" path — dropping happens only on the terminator.
//! 3. The hold is bounded twice over: [`MAX_HELD`] events, and the caller's short idle flush.
//!
//! The consequence for ordinary typing: a literal `Escape` press, `Escape` then `[`, `Alt+]` on its
//! own, and paste-like bursts are all delivered unchanged (in order), at the cost of at most one
//! idle-poll of latency on the two opener keys and nothing at all on every other key.
//!
//! # What is deliberately *not* handled here
//!
//! The DSR colour-scheme report (`CSI ? 997 ; 1 n`, the answer to
//! [`crate::terminal_query::COLOR_SCHEME_QUERY`]) produces **zero** crossterm events — `parse_csi`'s
//! `?` arm (`parse.rs:179-183`) only terminates on a final `u` or `c`, so any other final byte
//! leaves the sequence buffered rather than emitted. There is nothing in the key stream to filter.
//! It is already neutralised in practice because `terminal_query` appends a Primary Device
//! Attributes request to every probe, whose reply's final `c` flushes crossterm's buffer and is
//! itself dropped by crossterm's own `EventFilter`.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Hard cap on how many events the machine may hold before it gives up and replays them.
///
/// The longest real frame is the opener (1–2 events) + `1`,`1`,`;` + `rgb:RRRR/GGGG/BBBB`
/// (18 events) + the terminator, i.e. 23–24. The cap is generous enough for a `#rrrrggggbbbb`
/// form with trailing space and still bounds the hold to a fraction of a screen line.
pub const MAX_HELD: usize = 48;

/// Where the machine is in the shredded `ESC ] 1 1 ; … terminator` frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nothing held; every event passes straight through.
    Idle,
    /// A bare `Esc` is held — this may be the split-chunk form of the OSC introducer.
    Esc,
    /// The introducer matched. `n` characters of the literal tail `1`,`1`,`;` have matched.
    Tail(u8),
    /// The tail matched. Discarding payload characters until the terminator.
    Payload,
    /// Inside the payload, a bare `Esc` arrived — a split `ST` terminator may be completing.
    PayloadEsc,
}

/// The literal that must follow the introducer for an OSC **11** reply: `1`, `1`, `;`.
const TAIL: [char; 3] = ['1', '1', ';'];

/// A state machine over the crossterm event stream that removes a late OSC 11 background-colour
/// reply and passes everything else through untouched.
///
/// Feed it with [`push`](Self::push) and, when the input goes idle, [`flush`](Self::flush). Both
/// append the events to forward to a caller-owned buffer, so the pass-through path allocates
/// nothing.
#[derive(Debug)]
pub struct StrayReplyFilter {
    state: State,
    held: Vec<Event>,
}

impl Default for StrayReplyFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl StrayReplyFilter {
    /// A filter holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self { state: State::Idle, held: Vec::new() }
    }

    /// Whether events are currently being held. The reader thread uses this to shorten its poll
    /// timeout so a held opener is replayed promptly instead of waiting a full idle tick.
    #[must_use]
    pub fn is_holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// Feed one crossterm event, appending whatever should be forwarded to `out` (possibly nothing,
    /// possibly several events if a hold just collapsed).
    pub fn push(&mut self, ev: Event, out: &mut Vec<Event>) {
        // A completed hold is the ONLY path that discards anything; every other exit replays.
        match self.step(ev) {
            Step::Forward(ev) => out.push(ev),
            Step::Hold => {}
            Step::Swallowed => self.held.clear(),
            Step::Replay(ev) => {
                out.append(&mut self.held);
                out.push(ev);
            }
            Step::ReplayThenRetry(ev) => {
                out.append(&mut self.held);
                self.state = State::Idle;
                // Re-offer the event to a now-idle machine: `Esc` `Esc` must release the first and
                // hold the second, not drop either.
                match self.step(ev) {
                    Step::Forward(ev) | Step::Replay(ev) | Step::ReplayThenRetry(ev) => {
                        out.append(&mut self.held);
                        out.push(ev);
                    }
                    Step::Hold => {}
                    Step::Swallowed => self.held.clear(),
                }
            }
        }
    }

    /// Release everything held — the reader thread calls this when input goes idle, and it is what
    /// makes a lone `Escape` or a lone `Alt+]` reach the app.
    pub fn flush(&mut self, out: &mut Vec<Event>) {
        out.append(&mut self.held);
        self.state = State::Idle;
    }

    /// One transition. Never touches `self.held` except to append a newly-held event.
    ///
    /// Every arm that returns [`Step::Replay`] **must** disarm the machine first: a replay means the
    /// held prefix was not a reply, so leaving `state` armed would let the next events be re-held
    /// from a state they never earned and then discarded by a terminator alone. That is the one way
    /// this filter could eat a real keystroke, so the reset lives here rather than at the call site,
    /// where it would have to be repeated in [`Self::push`]'s outer *and* inner match.
    fn step(&mut self, ev: Event) -> Step {
        let Event::Key(key) = ev else {
            // Resize / focus / bracketed paste can never be part of an escape reply.
            if self.state == State::Idle {
                return Step::Forward(ev);
            }
            self.state = State::Idle;
            return Step::Replay(ev);
        };
        // crossterm only reports Release/Repeat under keyboard-enhancement flags cyrup does not
        // push, but if one appears it cannot be part of a reply frame either.
        if key.kind != KeyEventKind::Press {
            if self.state == State::Idle {
                return Step::Forward(Event::Key(key));
            }
            self.state = State::Idle;
            return Step::Replay(Event::Key(key));
        }
        if self.held.len() >= MAX_HELD {
            return Step::ReplayThenRetry(Event::Key(key));
        }
        match self.state {
            State::Idle => self.step_idle(key),
            State::Esc => self.step_esc(key),
            State::Tail(n) => self.step_tail(n, key),
            State::Payload => self.step_payload(key),
            State::PayloadEsc => self.step_payload_esc(key),
        }
    }

    fn step_idle(&mut self, key: KeyEvent) -> Step {
        // Form 1 (the common one): `ESC ]` collapsed by crossterm into a single `Alt+]`.
        if key.code == KeyCode::Char(']') && key.modifiers == KeyModifiers::ALT {
            self.state = State::Tail(0);
            self.held.push(Event::Key(key));
            return Step::Hold;
        }
        // Form 2: the reply was split at the `ESC` byte, so the introducer arrives as two events.
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            self.state = State::Esc;
            self.held.push(Event::Key(key));
            return Step::Hold;
        }
        Step::Forward(Event::Key(key))
    }

    fn step_esc(&mut self, key: KeyEvent) -> Step {
        if key.code == KeyCode::Char(']') && key.modifiers == KeyModifiers::NONE {
            self.state = State::Tail(0);
            self.held.push(Event::Key(key));
            return Step::Hold;
        }
        Step::ReplayThenRetry(Event::Key(key))
    }

    fn step_tail(&mut self, matched: u8, key: KeyEvent) -> Step {
        let expected = TAIL.get(usize::from(matched)).copied();
        if key.modifiers != KeyModifiers::NONE || Some(key.code) != expected.map(KeyCode::Char) {
            return Step::ReplayThenRetry(Event::Key(key));
        }
        self.held.push(Event::Key(key));
        let next = matched.saturating_add(1);
        self.state =
            if usize::from(next) == TAIL.len() { State::Payload } else { State::Tail(next) };
        Step::Hold
    }

    fn step_payload(&mut self, key: KeyEvent) -> Step {
        // `BEL` — crossterm's C0 arm turns 0x07 into `Ctrl+G`.
        if key.code == KeyCode::Char('g') && key.modifiers == KeyModifiers::CONTROL {
            self.state = State::Idle;
            return Step::Swallowed;
        }
        // `ST` (`ESC \`) collapsed into one `Alt+\`.
        if key.code == KeyCode::Char('\\') && key.modifiers == KeyModifiers::ALT {
            self.state = State::Idle;
            return Step::Swallowed;
        }
        // `ST` split across reads: the `\` decides on the next event.
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            self.state = State::PayloadEsc;
            self.held.push(Event::Key(key));
            return Step::Hold;
        }
        // Anything that is not an OSC 11 payload character aborts and replays. Uppercase hex digits
        // carry SHIFT (`char_code_to_event`, `parse.rs:127-133`), so SHIFT alone is tolerated.
        let modifiers_ok =
            key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT;
        let payload_char = matches!(key.code, KeyCode::Char(c) if is_osc11_payload_char(c));
        if !modifiers_ok || !payload_char {
            return Step::ReplayThenRetry(Event::Key(key));
        }
        self.held.push(Event::Key(key));
        Step::Hold
    }

    fn step_payload_esc(&mut self, key: KeyEvent) -> Step {
        if key.code == KeyCode::Char('\\') && key.modifiers == KeyModifiers::NONE {
            self.state = State::Idle;
            return Step::Swallowed;
        }
        Step::ReplayThenRetry(Event::Key(key))
    }
}

/// The alphabet a real OSC 11 payload can use — `#rrggbb`, `#rrrrggggbbbb`, `rgb:R/G/B` and the
/// `rgba:` variant (`terminal_query::parse_osc11_value`, Pi `terminal-colors.ts:35-65`), plus the
/// surrounding whitespace Pi's `value.trim()` tolerates. Anything else is real typing.
fn is_osc11_payload_char(c: char) -> bool {
    c.is_ascii_hexdigit() || matches!(c, '#' | ':' | '/' | ' ' | 'r' | 'g' | 'b' | 'R' | 'G' | 'B')
}

/// The outcome of one transition.
enum Step {
    /// Nothing is held; forward this event as-is.
    Forward(Event),
    /// The event joined the held prefix.
    Hold,
    /// The held prefix plus this event formed a complete OSC 11 frame — discard all of it.
    Swallowed,
    /// The held prefix was not a reply: replay it, then this event.
    Replay(Event),
    /// The held prefix was not a reply, and this event might start a NEW one: replay the prefix,
    /// then re-offer this event to an idle machine.
    ReplayThenRetry(Event),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    /// Exactly what crossterm 0.29 produces for one raw byte stream, so the tests below are written
    /// in terms of the bytes a terminal actually sends rather than hand-picked events.
    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, mods))
    }
    fn ch(c: char) -> Event {
        key(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn alt(c: char) -> Event {
        key(KeyCode::Char(c), KeyModifiers::ALT)
    }
    fn ctrl(c: char) -> Event {
        key(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn esc() -> Event {
        key(KeyCode::Esc, KeyModifiers::NONE)
    }

    /// Feed a whole burst with no idle gap and collect what the app would see.
    fn run(events: Vec<Event>) -> Vec<Event> {
        let mut f = StrayReplyFilter::new();
        let mut out = Vec::new();
        for ev in events {
            f.push(ev, &mut out);
        }
        // A burst always ends with the input going idle.
        f.flush(&mut out);
        out
    }

    /// The literal chars a user would see typed into the prompt if the frame leaked.
    fn typed(out: &[Event]) -> String {
        out.iter()
            .filter_map(|e| match e {
                Event::Key(KeyEvent { code: KeyCode::Char(c), .. }) => Some(*c),
                _ => None,
            })
            .collect()
    }

    /// The user's report, byte-exact: `ESC ] 1 1 ; r g b : 0 c 0 c / 0 b 0 b / 1 3 1 3 BEL`
    /// arriving in ONE read(2), which crossterm shreds into `Alt+]`, chars, `Ctrl+G`.
    fn shredded_bel_reply() -> Vec<Event> {
        let mut v = vec![alt(']'), ch('1'), ch('1'), ch(';')];
        v.extend("rgb:0c0c/0b0b/1313".chars().map(ch));
        v.push(ctrl('g'));
        v
    }

    #[test]
    fn a_late_osc11_reply_never_reaches_the_app() {
        let out = run(shredded_bel_reply());
        assert!(
            out.is_empty(),
            "the whole frame must be swallowed, got {out:?} (user would see {:?})",
            typed(&out)
        );
    }

    #[test]
    fn a_late_osc11_reply_terminated_by_st_is_swallowed_in_both_chunk_forms() {
        // `ST` in the same chunk: `ESC \` collapses into `Alt+\`.
        let mut v = vec![alt(']'), ch('1'), ch('1'), ch(';')];
        v.extend("#1e1e1e".chars().map(ch));
        v.push(alt('\\'));
        assert!(run(v).is_empty(), "Alt+\\ terminator must end the frame");

        // `ST` split at the `ESC`: two events.
        let mut v = vec![alt(']'), ch('1'), ch('1'), ch(';')];
        v.extend("#1e1e1e".chars().map(ch));
        v.push(esc());
        v.push(ch('\\'));
        assert!(run(v).is_empty(), "split ST terminator must end the frame");
    }

    #[test]
    fn a_reply_split_at_the_esc_byte_is_swallowed_too() {
        // Opener arrives as TWO events with no ALT modifier.
        let mut v = vec![esc(), ch(']'), ch('1'), ch('1'), ch(';')];
        v.extend("rgb:ffff/ffff/ffff".chars().map(ch));
        v.push(ctrl('g'));
        assert!(run(v).is_empty(), "the split-opener form must be recognised");
    }

    #[test]
    fn uppercase_hex_payload_carrying_shift_is_still_swallowed() {
        let mut v = vec![alt(']'), ch('1'), ch('1'), ch(';')];
        v.extend("rgb:".chars().map(ch));
        // `parse.rs:127-133` sets SHIFT on uppercase characters.
        v.extend("FFFF".chars().map(|c| key(KeyCode::Char(c), KeyModifiers::SHIFT)));
        v.extend("/0000/0000".chars().map(ch));
        v.push(ctrl('g'));
        assert!(run(v).is_empty(), "uppercase hex channels must not break the match");
    }

    // ---------------------------------------------------------------- keystroke safety ----

    #[test]
    fn a_literal_escape_press_is_delivered_unchanged() {
        let out = run(vec![esc()]);
        assert_eq!(out, vec![esc()], "a lone Escape must still reach the app");
    }

    #[test]
    fn escape_followed_by_a_bracket_is_delivered_unchanged() {
        // Both bracket kinds: `[` is not the OSC introducer at all, `]` is — and even `]` must
        // survive, because the tail `1`,`1`,`;` does not follow.
        assert_eq!(run(vec![esc(), ch('[')]), vec![esc(), ch('[')]);
        assert_eq!(run(vec![esc(), ch(']')]), vec![esc(), ch(']')]);
        assert_eq!(run(vec![esc(), ch(']'), ch('x')]), vec![esc(), ch(']'), ch('x')]);
    }

    #[test]
    fn a_genuine_alt_bracket_press_is_delivered_unchanged() {
        // The ambiguity that forbids committing on the opener: `Alt+]` typed by a human, and the
        // Kitty `ESC [ 93 ; 3 u` encoding of the same key, both produce this exact event.
        assert_eq!(run(vec![alt(']')]), vec![alt(']')]);
        assert_eq!(run(vec![alt(']'), ch('a')]), vec![alt(']'), ch('a')]);
    }

    #[test]
    fn a_double_escape_releases_the_first_and_holds_the_second() {
        assert_eq!(run(vec![esc(), esc()]), vec![esc(), esc()], "both Escapes must arrive, in order");
        assert_eq!(run(vec![esc(), esc(), esc()]), vec![esc(), esc(), esc()]);
    }

    #[test]
    fn typing_that_looks_like_the_prefix_but_is_not_terminated_is_replayed_in_order() {
        // A user really typing `Alt+]` `1` `1` `;` `a` `b`: the machine commits to the payload
        // state, then `a`… wait — `a` and `b` ARE hex digits, so the frame is still held. The
        // proof that nothing is lost is the flush.
        let burst = vec![alt(']'), ch('1'), ch('1'), ch(';'), ch('a'), ch('b')];
        assert_eq!(run(burst.clone()), burst, "an unterminated frame must be replayed whole");

        // And a payload character outside the OSC alphabet aborts immediately.
        let burst = vec![alt(']'), ch('1'), ch('1'), ch(';'), ch('z')];
        assert_eq!(run(burst.clone()), burst);
    }

    #[test]
    fn a_fast_paste_like_burst_of_ordinary_typing_is_untouched() {
        let burst: Vec<Event> =
            "the quick brown fox; jumps over 11 lazy dogs -- 1;2;3 [x] {y} ]z]".chars().map(ch).collect();
        assert_eq!(run(burst.clone()), burst, "ordinary typing must pass through byte-for-byte");
    }

    #[test]
    fn control_keys_inside_a_hold_are_replayed_not_eaten() {
        // Ctrl+C arriving while the opener is held must still reach the app.
        let burst = vec![alt(']'), ctrl('c')];
        assert_eq!(run(burst.clone()), burst);
        // So must Enter, arrows and every other non-Char key.
        let burst = vec![esc(), key(KeyCode::Enter, KeyModifiers::NONE)];
        assert_eq!(run(burst.clone()), burst);
        let burst = vec![alt(']'), ch('1'), ch('1'), ch(';'), key(KeyCode::Up, KeyModifiers::NONE)];
        assert_eq!(run(burst.clone()), burst);
    }

    #[test]
    fn non_key_events_flush_the_hold_and_pass_through() {
        let burst = vec![esc(), Event::Resize(80, 24)];
        assert_eq!(run(burst.clone()), burst);
        let burst = vec![alt(']'), Event::Paste("hello".into())];
        assert_eq!(run(burst.clone()), burst);
    }

    /// A non-key event (or a non-`Press` key) arriving mid-hold replays the prefix — and must also
    /// *disarm* the machine. If it did not, the machine would sit in `Payload` with an EMPTY hold,
    /// re-hold whatever the user typed next from a state those keys never earned, and then let a
    /// bare terminator discard them. That is a lost keystroke, the one outcome this filter must
    /// never produce.
    ///
    /// Drives the exact sequence from the review: `Alt+]` `1` `1` `;` (armed, 4 held) → `Resize`
    /// (replay) → `b` `e` `e` `f` (all legal OSC 11 hex, so all re-holdable) → `Ctrl+G` (BEL).
    #[test]
    fn a_replay_disarms_so_later_keys_cannot_be_swallowed_by_a_bare_terminator() {
        /// The interleaved event, plus the keys typed after it — none of which may be lost.
        fn assert_survives(interrupt: Event) {
            let mut f = StrayReplyFilter::new();
            let mut out = Vec::new();
            for ev in [alt(']'), ch('1'), ch('1'), ch(';')] {
                f.push(ev, &mut out);
            }
            assert!(out.is_empty(), "the prefix is held, not forwarded");

            f.push(interrupt.clone(), &mut out);
            assert_eq!(
                out,
                vec![alt(']'), ch('1'), ch('1'), ch(';'), interrupt.clone()],
                "the held prefix is replayed in order ahead of {interrupt:?}"
            );
            out.clear();

            // Every one of these is in the OSC 11 payload alphabet, so a still-armed machine would
            // hold them; `Ctrl+G` is BEL, so it would then swallow the lot.
            let typing = vec![ch('b'), ch('e'), ch('e'), ch('f'), ctrl('g')];
            for ev in typing.clone() {
                f.push(ev, &mut out);
            }
            f.flush(&mut out);
            assert_eq!(
                out, typing,
                "keys typed after {interrupt:?} were never preceded by an opener and must survive"
            );
        }

        // Both reachable non-key events: `Resize` is always live and `Paste` arrives because
        // `EnableBracketedPaste` is pushed (`App::into_stdout`).
        assert_survives(Event::Resize(80, 24));
        assert_survives(Event::Paste("hello".into()));
        // And the second `Step::Replay` producer: a key whose kind is not `Press`.
        assert_survives(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )));
    }

    /// The same leak, reached through the *split* opener and an `ST` terminator, and asserted on the
    /// machine's own state rather than only on the output — a hold that is armed with nothing held
    /// is the invariant violation itself.
    #[test]
    fn a_replay_leaves_the_machine_idle_not_armed_and_empty() {
        for prefix in [
            vec![esc()],                                     // State::Esc
            vec![alt(']')],                                  // State::Tail(0)
            vec![esc(), ch(']'), ch('1')],                   // State::Tail(1)
            vec![alt(']'), ch('1'), ch('1'), ch(';')],       // State::Payload
            vec![alt(']'), ch('1'), ch('1'), ch(';'), esc()], // State::PayloadEsc
        ] {
            let mut f = StrayReplyFilter::new();
            let mut out = Vec::new();
            for ev in prefix.clone() {
                f.push(ev, &mut out);
            }
            f.push(Event::Resize(80, 24), &mut out);
            assert_eq!(
                (f.state, f.held.as_slice()),
                (State::Idle, [].as_slice()),
                "after replaying {prefix:?} the machine must be disarmed, not armed-and-empty"
            );

            // The observable consequence: a lone terminator now passes through.
            out.clear();
            f.push(alt('\\'), &mut out);
            f.flush(&mut out);
            assert_eq!(out, vec![alt('\\')], "a bare ST after a replay is real typing");
        }
    }

    #[test]
    fn the_hold_is_bounded_and_gives_up_by_replaying_everything() {
        // A payload that never terminates: every character is a legal hex digit, so only the cap
        // can stop it. Nothing may be lost when it does.
        let mut burst = vec![alt(']'), ch('1'), ch('1'), ch(';')];
        burst.extend(std::iter::repeat_n(ch('a'), MAX_HELD * 3));
        let out = run(burst.clone());
        assert_eq!(out, burst, "every held event must be replayed when the cap is hit");

        // And the machine is idle afterwards, so a REAL reply right after still gets swallowed.
        let mut f = StrayReplyFilter::new();
        let mut out = Vec::new();
        for ev in burst {
            f.push(ev, &mut out);
        }
        out.clear();
        for ev in shredded_bel_reply() {
            f.push(ev, &mut out);
        }
        assert!(out.is_empty(), "the machine must recover after giving up, got {out:?}");
    }

    #[test]
    fn holding_never_exceeds_the_cap() {
        let mut f = StrayReplyFilter::new();
        let mut out = Vec::new();
        f.push(alt(']'), &mut out);
        f.push(ch('1'), &mut out);
        f.push(ch('1'), &mut out);
        f.push(ch(';'), &mut out);
        for _ in 0..1000 {
            f.push(ch('a'), &mut out);
            assert!(f.held.len() <= MAX_HELD, "hold must stay bounded: {}", f.held.len());
        }
    }

    #[test]
    fn an_idle_gap_mid_frame_releases_rather_than_eats() {
        // Interleaving a flush (the reader thread's idle tick) with a real frame degrades to the
        // OLD behaviour — the leak — rather than to lost input. That is the safe direction.
        let mut f = StrayReplyFilter::new();
        let mut out = Vec::new();
        f.push(alt(']'), &mut out);
        f.flush(&mut out);
        assert_eq!(out, vec![alt(']')], "an idle opener is released, never dropped");
    }

    #[test]
    fn every_event_of_a_non_matching_burst_is_accounted_for() {
        // Property-ish sweep: for a set of bursts that contain NO complete frame, output == input.
        let alphabet = [alt(']'), esc(), ch(']'), ch('1'), ch(';'), ch('a'), ch('z'), ctrl('c')];
        for i in 0..alphabet.len() {
            for j in 0..alphabet.len() {
                for k in 0..alphabet.len() {
                    let burst =
                        vec![alphabet[i].clone(), alphabet[j].clone(), alphabet[k].clone()];
                    let out = run(burst.clone());
                    assert_eq!(out, burst, "no 3-event burst without a terminator may be altered");
                }
            }
        }
    }
}
