//! The startup package-update notification block — Pi `showPackageUpdateNotification`
//! (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3920-3936`), shown when the
//! detached check fired at `:850-856` settles with a non-empty list.
//!
//! The binary spawns that check (`cyrup::update_check::spawn_package_update_check`), hands the
//! receiver to `App::set_package_update_channel`, and `App::run`'s arm calls the method under test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::{App, UiTheme};
use ratatui::backend::TestBackend;

fn all_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out.push_str(&app.scrollback_text());
    out
}

/// The notification names every out-of-date package and the command that fixes them.
#[test]
fn the_notice_lists_the_packages_and_the_update_command() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.transcript_mut().push_package_updates(&[
        "github.com/nicobailon/pi-intercom".to_string(),
        "github.com/MasuRii/pi-permission-system".to_string(),
    ]);
    app.draw().unwrap();

    let text = all_text(&app);
    assert!(
        text.contains("Package Updates Available"),
        "no notification title:\n{text}"
    );
    assert!(
        text.contains("cyrup update --extensions"),
        "the notice does not name the command that acts on it:\n{text}"
    );
    assert!(
        text.contains("- github.com/nicobailon/pi-intercom"),
        "first package missing from the notice:\n{text}"
    );
    assert!(
        text.contains("- github.com/MasuRii/pi-permission-system"),
        "second package missing from the notice:\n{text}"
    );
}

/// MIRROR: an EMPTY list draws nothing at all — upstream never calls the notifier unless
/// `updates.length > 0` (`:851`), so a healthy install must stay silent.
#[test]
fn an_empty_list_shows_no_notice() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.transcript_mut().push_package_updates(&[]);
    app.draw().unwrap();

    let text = all_text(&app);
    assert!(
        !text.contains("Package Updates"),
        "an empty update list still drew a notification:\n{text}"
    );
}

/// The channel seam the binary uses. Installing `None` (the network policy declined) must be
/// accepted and must leave the run loop with nothing to wait on.
#[test]
fn the_update_channel_accepts_both_a_receiver_and_none() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.set_package_update_channel(None);
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
    app.set_package_update_channel(Some(rx));
    app.draw().unwrap();
    // Nothing has been sent, so nothing is on screen yet.
    assert!(!all_text(&app).contains("Package Updates"));
}
