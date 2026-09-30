//! SEAM-134 — the startup `--resume` picker opens at once and streams its listing in.
//!
//! ```ts
//! // pi v0.87.1 cli/session-picker.ts:15-55 — `selectSession` hands the component two loaders;
//! // session-selector.ts:869 (constructor) `void this.loadScope("current")`, :956-988 progress.
//! ```
//!
//! cyrup scanned every session file synchronously before the picker existed (`prelaunch.rs` →
//! `gather_session_scopes`), and `run_startup_selector`'s loop could not have taken a report anyway:
//! it parked in a blocking 3600 s read. These drive [`StartupLoop`] — the same loop the terminal
//! runs, over a `TestBackend` and a scripted key source — against a listing that BLOCKS until
//! released (one session "file" is a FIFO, see `resume_streamed_listing::Blocker`), so what the
//! screen shows while the scan is stuck is a fact, not a race.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cyrup_session_svc::SessionListing;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::resume_streamed_listing::{Blocker, fixture, session_dir, session_lines};
use crate::error::TuiError;
use crate::keymap::SelectKeymap;
use crate::selector::SelectorOutcome;
use crate::session_selector::SessionSelector;
use crate::startup_loop::{LoadDriver, StartupEvents, StartupLoop, StartupSessionLoads};
use crate::theme::UiTheme;

/// A key source that answers each wait with the next scripted event, then with `repeat` (or a
/// timeout, when that is `None`). It sleeps a little per call, as a real poll would.
///
/// The loop under test never yields to the runtime (its key wait is a blocking one, as on a real
/// terminal), so a `tokio::time::timeout` around it cannot cut it short: the source ends the run
/// itself, with an error, once `give_up` passes.
struct Script {
    queue: VecDeque<Event>,
    repeat: Option<Event>,
    give_up: Instant,
}

impl Script {
    fn new(repeat: Option<Event>) -> Self {
        Self {
            queue: VecDeque::new(),
            repeat,
            give_up: Instant::now() + Duration::from_secs(10),
        }
    }
}

impl StartupEvents for Script {
    fn next(&mut self, wait: Duration) -> Result<Option<Event>, TuiError> {
        if let Some(ev) = self.queue.pop_front() {
            return Ok(Some(ev));
        }
        if Instant::now() > self.give_up {
            return Err(TuiError::Backend("the script ran out of time".to_string()));
        }
        std::thread::sleep(wait.min(Duration::from_millis(5)));
        Ok(self.repeat.clone())
    }
}

fn enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

fn screen(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)))
                .map(|c| c.symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn loads(fx: &super::resume_streamed_listing::Fixture) -> StartupSessionLoads {
    let dir = session_dir(fx);
    let elsewhere = fx.cwd.parent().unwrap().join("no-such-sessions");
    StartupSessionLoads {
        current: SessionListing::Dir {
            dir,
            cwd_filter: None,
        },
        all: SessionListing::Dir {
            dir: elsewhere,
            cwd_filter: None,
        },
    }
}

async fn noop(_: &str) {}

/// The row's Verify, end to end. The picker is on screen, in its loading state, while the scan is
/// still parked on the FIFO; the first batch lands before the second; releasing the scan completes
/// it.
///
/// **Red without the fix:** nothing serves the picker's load request — the old loop had no channel
/// — so the list stays `Loading ...` and empty for ever and the `newest session` wait below times
/// out. (The old `--resume` path instead ran the scan before any frame: `released_early` would be
/// `true` by the time a picker existed.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_frame_is_loading_and_rows_arrive_as_progress() {
    let fx = fixture();
    let mut blocker = Blocker::new(&session_dir(&fx), &fx.cwd);
    let mut picker = SessionSelector::new(vec![]).with_async_loaders();
    let mut terminal = Terminal::new(TestBackend::new(140, 34)).unwrap();
    let mut events = Script::new(None);
    let keymap = SelectKeymap::default();
    let theme = UiTheme::dark();
    let mut lp = StartupLoop {
        terminal: &mut terminal,
        events: &mut events,
        theme: &theme,
        keymap: &keymap,
        inner: &mut picker,
        loads: Some(LoadDriver::new(loads(&fx))),
    };

    // Turn one: the picker is painted before any report can have been applied.
    assert!(lp.step(&mut noop).await.unwrap().is_none());
    assert!(
        !blocker.released_early(),
        "the picker waited for the listing"
    );
    let first = screen(lp.terminal);
    assert!(first.contains("Loading ..."), "{first}");
    assert!(!first.contains("the newest session"), "{first}");

    // The first batch (pi publishes after the first file).
    let deadline = Instant::now() + Duration::from_secs(10);
    let view = loop {
        assert!(lp.step(&mut noop).await.unwrap().is_none());
        let view = screen(lp.terminal);
        if view.contains("the newest session") {
            break view;
        }
        assert!(Instant::now() < deadline, "no rows arrived:\n{view}");
    };
    assert!(view.contains("Loading 1/2"), "{view}");
    assert!(!view.contains("the blocked session"), "{view}");

    blocker.release().await;
    let view = loop {
        assert!(lp.step(&mut noop).await.unwrap().is_none());
        let view = screen(lp.terminal);
        if view.contains("the blocked session") {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "the listing never finished:\n{view}"
        );
    };
    assert!(!view.contains("Loading"), "{view}");
}

/// A row that has arrived is selectable while the rest of the scan is still stuck — and the choice
/// carries the stored cwd out, which `prelaunch` needs for the missing-cwd check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streamed_row_can_be_picked_while_the_scan_is_stuck() {
    let fx = fixture();
    let mut blocker = Blocker::new(&session_dir(&fx), &fx.cwd);
    let mut picker = SessionSelector::new(vec![]).with_async_loaders();
    let mut terminal = Terminal::new(TestBackend::new(140, 34)).unwrap();
    // Enter is ignored by an empty list; it confirms the moment the first batch has landed.
    let mut events = Script::new(Some(enter()));
    let keymap = SelectKeymap::default();
    let theme = UiTheme::dark();
    let lp = StartupLoop {
        terminal: &mut terminal,
        events: &mut events,
        theme: &theme,
        keymap: &keymap,
        inner: &mut picker,
        loads: Some(LoadDriver::new(loads(&fx))),
    };
    let (outcome, cwds) = lp.run(noop).await.expect("no row ever arrived");
    assert!(
        !blocker.released_early(),
        "the pick waited for the whole listing"
    );
    let SelectorOutcome::Confirm(path) = outcome else {
        panic!("expected a confirm, got {outcome:?}");
    };
    assert!(path.ends_with("_aaaa.jsonl"), "{path}");
    assert_eq!(
        cwds.get(&path).map(String::as_str),
        Some(fx.cwd.display().to_string().as_str())
    );
    blocker.release().await;
}

/// pi `refreshSessionsAfterMutation` (`session-selector.ts:1024-1029`): after a rename the scope on
/// screen is listed again, so the picker shows what is on disk.
///
/// **Red without the reload:** the driver never starts the requested load, the header stays
/// settled, and no report arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_reloads_the_listing() {
    let fx = fixture();
    let dir = session_dir(&fx);
    let target = dir.join("2026-08-09T10-00-00-000Z_eeee.jsonl");
    std::fs::write(
        &target,
        session_lines(
            "01890000-0000-7000-8000-00000000eeee",
            &fx.cwd,
            "a session to rename",
        ),
    )
    .unwrap();
    let mut picker = SessionSelector::new(vec![]).with_async_loaders();
    let mut driver = LoadDriver::new(loads(&fx));
    let deadline = Instant::now() + Duration::from_secs(10);
    driver.serve(&mut picker);
    while picker.load_status().0 {
        driver.drain(&mut picker);
        assert!(Instant::now() < deadline, "the first load never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    cyrup_session_svc::rename_session_file_at(&target, "renamed on disk").unwrap();
    driver.refresh_after_mutation(&mut picker);
    assert_eq!(
        picker.load_status(),
        (true, None),
        "the scope was not reloaded"
    );
    while picker.load_status().0 {
        driver.drain(&mut picker);
        assert!(Instant::now() < deadline, "the reload never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
