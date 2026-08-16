use crate::app::map_event;
use crate::component::InputEvent;
use crate::escape_reassembly::EscapeReassembler;
use crate::stray_reply::StrayReplyFilter;
use ratatui::crossterm::event::Event;

/// The production input pipeline end-to-end: what [`crossterm_input_stream`]'s reader thread does
/// to a burst of raw crossterm events, i.e. [`EscapeReassembler`] then [`StrayReplyFilter`] then
/// [`map_event`], with the idle flush at the end of the burst.
#[cfg(test)]
fn input_pipeline(raw: Vec<Event>) -> Vec<InputEvent> {
    let mut reassembler = EscapeReassembler::new();
    let mut filter = StrayReplyFilter::new();
    let mut reassembled: Vec<Event> = Vec::new();
    let mut released: Vec<Event> = Vec::new();
    let mut out: Vec<InputEvent> = Vec::new();
    for ev in raw {
        reassembler.push(ev, &mut reassembled);
        for ev in reassembled.drain(..) {
            filter.push(ev, &mut released);
        }
        out.extend(released.drain(..).filter_map(map_event));
    }
    // Input has gone quiet: the reader thread's `Ok(false)` poll arm.
    reassembler.flush(&mut reassembled);
    for ev in reassembled.drain(..) {
        filter.push(ev, &mut released);
    }
    filter.flush(&mut released);
    out.extend(released.drain(..).filter_map(map_event));
    out
}


#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod stray_reply_pipeline_tests {
    use crate::app::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use super::input_pipeline;
    use crate::UiTheme;
    use crate::InputEvent;
    use ratatui::crossterm::event::Event;

    fn ch(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    /// A user launched cyrup and got `11;rgb:0c0c/0b0b/1313` typed into their prompt: the terminal
    /// answered the boot OSC 11 probe after `terminal_query`'s 100 ms deadline, so the reply reached
    /// the crossterm reader and was shredded into keys. Drive the exact shredded burst through the
    /// real reader-thread pipeline and then through the real editor, and assert the prompt is empty.
    #[test]
    fn a_late_osc11_reply_never_reaches_the_editor() {
        let mut raw = vec![Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT))];
        raw.extend("11;rgb:0c0c/0b0b/1313".chars().map(ch));
        // BEL (0x07) reaches crossterm's C0 arm as Ctrl+G.
        raw.push(Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)));

        let delivered = input_pipeline(raw);
        assert!(delivered.is_empty(), "no input event may survive the frame, got {delivered:?}");

        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        for ev in &delivered {
            app.handle_input(ev);
        }
        assert_eq!(app.state().editor.text(), "", "the prompt must be untouched");
    }

    /// TUI-045's own Verify, at the pipeline level: "drive `input_pipeline` with the two-chunk form
    /// and assert one `Up` arrives rather than `Esc` + `[` + `A`."
    ///
    /// RED before [`crate::escape_reassembly`] existed — this produced exactly `Esc`, `Char('[')`,
    /// `Char('A')`, which at idle types `[A` into the prompt and mid-stream aborts the running turn
    /// (reproduced live on 2026-08-13 with two `tmux send-keys -H` writes 60 ms apart).
    #[test]
    fn an_arrow_key_split_at_the_esc_byte_reaches_the_app_as_one_up() {
        // What crossterm emits when a read ends on `0x1b` and the next read carries `[A`.
        let raw = vec![
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            ch('['),
            Event::Key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
        ];
        let delivered = input_pipeline(raw);
        // `InputEvent` is not `PartialEq`, so match the shape (the same style the sibling test uses).
        match delivered.as_slice() {
            [InputEvent::Key(k)] => {
                assert_eq!(*k, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
            }
            other => panic!("the split arrow must reassemble to one Up, got {other:?}"),
        }

        // And nothing lands in the prompt.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        for ev in &delivered {
            app.handle_input(ev);
        }
        assert_eq!(app.state().editor.text(), "", "no `[A` may be typed into the prompt");
    }

    /// The safety half: the same pipeline must deliver ordinary typing byte-for-byte, including the
    /// two keys the filter is allowed to hold (`Escape` and `Alt+]`).
    #[test]
    fn ordinary_typing_survives_the_pipeline_intact() {
        let mut raw: Vec<Event> = "hello 11; world".chars().map(ch).collect();
        raw.push(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        raw.push(Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)));

        let delivered = input_pipeline(raw.clone());
        assert_eq!(delivered.len(), raw.len(), "every key must be delivered: {delivered:?}");
        for (i, (got, want)) in delivered.iter().zip(raw.iter()).enumerate() {
            match (got, want) {
                (InputEvent::Key(a), Event::Key(b)) => assert_eq!(a, b, "event {i} differs"),
                other => panic!("event {i} changed shape: {other:?}"),
            }
        }

        // And it lands in the editor as the literal text the user typed.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        for ev in &delivered {
            app.handle_input(ev);
        }
        assert_eq!(app.state().editor.text(), "hello 11; world");
    }
}
