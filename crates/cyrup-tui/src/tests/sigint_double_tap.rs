#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod ctrl_c_tests {
    use crate::InputEvent;
    use crate::UiTheme;
    use crate::app::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn ctrl_c() -> InputEvent {
        InputEvent::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
    }

    /// F10 — Pi `handleCtrlC` (interactive-mode.ts:3797-3805): a second Ctrl+C within 500 ms exits,
    /// with NO emptiness gate (the first press clears the editor and records the time even when the
    /// buffer is non-empty; only the timing — not emptiness — gates the exit).
    #[test]
    fn double_ctrl_c_within_500ms_exits_regardless_of_editor_contents() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        // A NON-empty editor: the first Ctrl+C still only clears (no exit) — disproving the old
        // "exit when already empty" mis-statement by showing the inverse: contents don't force a clear
        // vs exit choice, timing does.
        app.editor_mut().set_text("some draft text");
        assert_eq!(
            app.handle_input(&ctrl_c()),
            AppAction::Redraw,
            "first Ctrl+C clears, never exits"
        );
        assert_eq!(
            app.state().editor.text(),
            "",
            "first Ctrl+C cleared the buffer"
        );
        assert!(!app.state().should_quit, "one press must not exit");
        // Immediate second press (well within 500 ms) → exit.
        assert_eq!(
            app.handle_input(&ctrl_c()),
            AppAction::Quit,
            "second Ctrl+C within 500 ms exits"
        );
        assert!(app.state().should_quit, "double-tap sets the quit flag");
    }

    /// A lone Ctrl+C on an EMPTY editor does NOT exit (there is no emptiness gate), and a press that
    /// lands MORE than 500 ms after the previous one re-clears + re-arms rather than exiting.
    #[test]
    fn single_or_stale_ctrl_c_does_not_exit() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        // Empty editor, single press → clear/record, NOT exit (the mis-stated rule would have exited).
        assert_eq!(
            app.handle_input(&ctrl_c()),
            AppAction::Redraw,
            "empty single Ctrl+C must not exit"
        );
        assert!(!app.state().should_quit);
        // Age the recorded press beyond the 500 ms window; the next press is a fresh first tap.
        app.state_mut().last_sigint =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(600));
        assert_eq!(
            app.handle_input(&ctrl_c()),
            AppAction::Redraw,
            "a >500 ms-later Ctrl+C re-arms"
        );
        assert!(
            !app.state().should_quit,
            "outside the window is not a double-tap"
        );
    }
}
