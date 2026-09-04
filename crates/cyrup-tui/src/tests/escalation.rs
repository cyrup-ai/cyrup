#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tui092_wedge_watchdog_tests {
    //! Unit coverage for the TUI-092 escape-hatch machinery: the escalate-chord recogniser, the
    //! press-driven [`Escalation`] ladder, and the [`ArmGuard`] overrun recorder. The chord #3
    //! hard-exit leg (`presses >= PANIC_PRESSES` → `std::process::exit(130)`) is deliberately NOT
    //! exercised in-process — it would terminate the test runner; the task file's definition of
    //! done verifies it in a real terminal.
    use crate::app::*;
    use cyrup_core::CancelToken;
    use ratatui::crossterm::event::KeyEvent;
    use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    use std::time::Duration;

    /// Serialises every test that touches the process-global watchdog state (`INPUT_SERVICED`,
    /// `TERMINAL_RELEASED`, `ACTIVE_ARM`, `OVER_BUDGET_ARM`) — the test harness runs this module's
    /// tests on parallel threads, and an unsynchronised `mark_input_serviced` from a neighbour
    /// would read as "the loop serviced input" here.
    static GLOBAL_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn global_state_lock() -> std::sync::MutexGuard<'static, ()> {
        GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ctrl(code: KeyCode, kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent::new_with_kind(code, KeyModifiers::CONTROL, kind))
    }

    /// A chord the loop was then shown not to have serviced, `gap` ago — the state an armed
    /// ladder is in when the next chord arrives.
    fn armed_unserviced(gap: Duration, presses: u32) -> Escalation {
        Escalation::Armed {
            serviced: input_serviced(),
            last: std::time::Instant::now() - gap,
            presses,
        }
    }

    // --- is_escalate_chord ----------------------------------------------------------------

    #[test]
    fn ctrl_c_and_ctrl_d_presses_are_escalate_chords() {
        assert!(is_escalate_chord(&ctrl(
            KeyCode::Char('c'),
            KeyEventKind::Press
        )));
        assert!(is_escalate_chord(&ctrl(
            KeyCode::Char('d'),
            KeyEventKind::Press
        )));
    }

    #[test]
    fn releases_and_repeats_are_not_escalate_chords() {
        // Load-bearing on Windows, where crossterm populates `kind` unconditionally: one physical
        // press arrives as a press AND a release, and counting both would burn two of
        // PANIC_PRESSES on a single chord.
        assert!(!is_escalate_chord(&ctrl(
            KeyCode::Char('c'),
            KeyEventKind::Release
        )));
        assert!(!is_escalate_chord(&ctrl(
            KeyCode::Char('d'),
            KeyEventKind::Release
        )));
        assert!(!is_escalate_chord(&ctrl(
            KeyCode::Char('c'),
            KeyEventKind::Repeat
        )));
    }

    #[test]
    fn other_keys_are_not_escalate_chords() {
        // No CONTROL modifier.
        assert!(!is_escalate_chord(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        ))));
        // CONTROL, but a different key.
        assert!(!is_escalate_chord(&ctrl(
            KeyCode::Char('x'),
            KeyEventKind::Press
        )));
        assert!(!is_escalate_chord(&ctrl(KeyCode::Esc, KeyEventKind::Press)));
        // Not a key event at all.
        assert!(!is_escalate_chord(&Event::Resize(80, 24)));
        assert!(!is_escalate_chord(&Event::Paste("c".into())));
    }

    // --- Escalation::holds_open ------------------------------------------------------------

    #[test]
    fn only_an_armed_ladder_holds_the_reader_open_past_cancel() {
        assert!(!Escalation::Idle.holds_open());
        assert!(armed_unserviced(Duration::ZERO, 1).holds_open());
    }

    // --- Escalation::on_press --------------------------------------------------------------

    #[test]
    fn the_first_chord_arms_without_cancelling() {
        let _lock = global_state_lock();
        let cancel = CancelToken::new();
        let next = Escalation::Idle.on_press(&cancel);
        // Chord #1 carries its own HEAD meaning (clear / forward-delete / quit) and must not be a
        // stage: no cancel, presses = 1, ladder armed against the CURRENT serviced count.
        assert!(!cancel.is_cancelled());
        let Escalation::Armed {
            serviced, presses, ..
        } = next
        else {
            panic!("expected the ladder to arm, got {next:?}");
        };
        assert_eq!(presses, 1);
        assert_eq!(serviced, input_serviced());
    }

    #[test]
    fn a_serviced_advance_between_chords_resets_the_ladder() {
        let _lock = global_state_lock();
        let cancel = CancelToken::new();
        let before = armed_unserviced(PANIC_MIN_GAP + Duration::from_millis(50), 1);
        // The run loop drained input since the chord that armed the ladder: it IS servicing, so
        // the chord already did its HEAD job and the ladder goes back to the bottom.
        mark_input_serviced();
        let next = before.on_press(&cancel);
        assert!(!cancel.is_cancelled());
        let Escalation::Armed {
            serviced, presses, ..
        } = next
        else {
            panic!("expected the ladder to re-arm at the bottom, got {next:?}");
        };
        assert_eq!(presses, 1);
        assert_eq!(serviced, input_serviced());
    }

    #[test]
    fn a_spaced_unserviced_second_chord_fires_the_cooperative_cancel() {
        let _lock = global_state_lock();
        let cancel = CancelToken::new();
        let before = armed_unserviced(PANIC_MIN_GAP + Duration::from_millis(50), 1);
        let next = before.on_press(&cancel);
        // Chord #2 against a loop that serviced nothing since chord #1: the cooperative half of
        // signals.rs's two-delivery contract.
        assert!(cancel.is_cancelled());
        let Escalation::Armed { presses, .. } = next else {
            panic!("expected the ladder to climb, got {next:?}");
        };
        assert_eq!(presses, 2);
    }

    #[test]
    fn auto_repeat_cannot_climb_the_ladder() {
        let _lock = global_state_lock();
        let cancel = CancelToken::new();
        // A terminal's key auto-repeat delivers a held Ctrl+D as genuine Press events at ~30ms
        // intervals — far inside PANIC_MIN_GAP. The ladder must not climb, and the cancel must
        // not fire, no matter how many arrive.
        let mut state = armed_unserviced(Duration::ZERO, 1);
        for _ in 0..10 {
            state = state.on_press(&cancel);
        }
        assert!(!cancel.is_cancelled());
        let Escalation::Armed { presses, .. } = state else {
            panic!("expected the ladder to stay armed, got {state:?}");
        };
        assert_eq!(presses, 1);
    }

    #[test]
    fn a_released_terminal_disarms_on_press() {
        let _lock = global_state_lock();
        let cancel = CancelToken::new();
        let _released = TerminalReleased::enter();
        // A by-design block (Ctrl+G editor / Ctrl+Z suspend) is indistinguishable from a wedge by
        // observation alone; the flag is what stops a working chord becoming an escalation.
        let next = armed_unserviced(PANIC_MIN_GAP + Duration::from_millis(50), 2).on_press(&cancel);
        assert!(!cancel.is_cancelled());
        assert!(matches!(next, Escalation::Idle));
    }

    // --- Escalation::tick ------------------------------------------------------------------

    #[test]
    fn tick_disarms_once_the_loop_services_input_again() {
        let _lock = global_state_lock();
        let before = armed_unserviced(PANIC_MIN_GAP + Duration::from_millis(50), 2);
        mark_input_serviced();
        assert!(matches!(before.tick(), Escalation::Idle));
    }

    #[test]
    fn tick_disarms_while_the_terminal_is_released() {
        let _lock = global_state_lock();
        let _released = TerminalReleased::enter();
        let before = armed_unserviced(PANIC_MIN_GAP + Duration::from_millis(50), 2);
        assert!(matches!(before.tick(), Escalation::Idle));
    }

    #[test]
    fn tick_never_promotes_and_idle_is_a_fixed_point() {
        let _lock = global_state_lock();
        assert!(matches!(Escalation::Idle.tick(), Escalation::Idle));
        // Nothing serviced, terminal not released: the ladder holds its position — promotion is
        // driven by presses, never by elapsed time.
        let before = armed_unserviced(Duration::from_secs(60), 2);
        let Escalation::Armed { presses, .. } = before.tick() else {
            panic!("expected the ladder to hold, got {:?}", before.tick());
        };
        assert_eq!(presses, 2);
    }

    // --- ArmGuard --------------------------------------------------------------------------

    #[test]
    fn an_in_budget_arm_is_recorded_while_active_and_cleared_on_drop() {
        let _lock = global_state_lock();
        // Start from a clean slate in case a neighbour left residue.
        let _ = OVER_BUDGET_ARM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        {
            let _arm = ArmGuard::enter("test_arm");
            let slot = ACTIVE_ARM
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some((arm, _since)) = *slot else {
                panic!("expected ACTIVE_ARM to name the entered arm");
            };
            assert_eq!(arm, "test_arm");
        }
        // Drop cleared the active slot, and an arm inside ARM_BUDGET records no overrun.
        let slot = ACTIVE_ARM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(slot.is_none());
        drop(slot);
        let over = OVER_BUDGET_ARM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(over.is_none());
    }

    #[test]
    fn an_over_budget_arm_is_recorded_on_drop_for_the_next_loop_iteration() {
        let _lock = global_state_lock();
        let _ = OVER_BUDGET_ARM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        {
            // Constructed directly (rather than via `enter`) so the start instant can be backdated
            // past ARM_BUDGET without an 8-second sleep; `enter`'s ACTIVE_ARM write is not the
            // behaviour under test here.
            let _arm = ArmGuard(
                "over_budget_test_arm",
                std::time::Instant::now() - ARM_BUDGET - Duration::from_secs(1),
            );
        }
        let mut over = OVER_BUDGET_ARM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(over.take(), Some("over_budget_test_arm"));
    }
}
