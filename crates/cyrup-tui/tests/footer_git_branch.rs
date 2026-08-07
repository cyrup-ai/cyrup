//! The footer's `(git branch)` segment, end to end — the port of Pi's `FooterDataProvider`
//! (`pi/packages/coding-agent/src/core/footer-data-provider.ts`) consumed by `footer.ts:116-120`.
//!
//! # What was broken
//!
//! `StatusLine::set_branch` existed and the location line already rendered `~/path (branch)` for it,
//! but NOTHING in cyrup ever resolved a git HEAD: `set_branch` had exactly two callers, both tests,
//! and the binary's footer seeding called only `set_cwd`. The segment was unreachable in a real
//! session no matter what repo you launched in.
//!
//! These tests drive the **assembled `App`** through the same call the binary's `seed_footer` makes
//! (`App::set_footer_git_cwd`, `crates/cyrup/src/main.rs`) and assert on the rendered footer, plus
//! the live-refresh path (`App::poll_footer_git_branch`) the run loop's poll tick drives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;

/// A private scratch dir; `name` keeps concurrent tests in this file off each other's files.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cyrup-footer-branch-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A `.git` directory holding `head` — enough for the footer, which READS HEAD rather than shelling
/// out (Pi `resolveGitBranchSync`). No `git` binary is involved, so the test cannot depend on one.
fn write_head(root: &Path, head: &str) {
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git").join("HEAD"), head).unwrap();
}

/// Only the live region — the bottom rows the app repaints, which is where the footer lives.
fn live_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let vh = app.viewport_height().min(area.height);
    let mut out = String::new();
    for y in (area.height - vh)..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// THE regression: seeding the footer from a cwd inside a git repo puts the branch on screen.
///
/// Mirrors the binary exactly — `status.set_cwd(home_relative(cwd))` then
/// `app.set_footer_git_cwd(cwd)` (`crates/cyrup/src/main.rs`, `seed_footer`).
#[test]
fn seeding_from_a_repo_cwd_renders_the_branch_segment() {
    let repo = scratch("seeded");
    write_head(&repo, "ref: refs/heads/david/cyrup\n");

    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_cwd("~/src/cyrup");
    app.set_footer_git_cwd(&repo);
    app.draw().unwrap();

    let live = live_text(&app);
    assert!(
        live.contains("~/src/cyrup (david/cyrup)"),
        "footer location line has no branch segment:\n{live}"
    );
}

/// MIRROR (stays green with or without any git resolution at all): a cwd that is NOT in a repo must
/// render the bare cwd with no parenthesised segment — Pi's `if (branch)` guard, `footer.ts:118`.
///
/// This is what proves the assertion above is about the branch and not about the location line
/// merely existing.
#[test]
fn a_cwd_outside_any_repo_renders_no_branch_segment() {
    let plain = scratch("norepo");

    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_cwd("~/src/cyrup");
    app.set_footer_git_cwd(&plain);
    app.draw().unwrap();

    let live = live_text(&app);
    assert!(live.contains("~/src/cyrup"), "footer location line missing entirely:\n{live}");
    assert!(
        !live.contains("~/src/cyrup ("),
        "a non-repo cwd invented a branch segment:\n{live}"
    );
}

/// A detached HEAD reads as Pi's literal `"detached"` (`resolveGitBranchSync`'s `return "detached"`),
/// not as "no repo" and not as a raw sha.
#[test]
fn a_detached_head_renders_the_word_detached() {
    let repo = scratch("detached");
    write_head(&repo, "9f8c1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b\n");

    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_cwd("~/w");
    app.set_footer_git_cwd(&repo);
    app.draw().unwrap();

    let live = live_text(&app);
    assert!(live.contains("~/w (detached)"), "detached HEAD not shown:\n{live}");
    assert!(!live.contains("9f8c1a2b"), "raw sha leaked into the footer:\n{live}");
}

/// The live-refresh half: a `git checkout` in another terminal repaints the footer. This is the body
/// of the run loop's poll arm (`_ = git_branch_poll.tick(), if …in_repo() => …`), driven directly so
/// no wall-clock timing is involved.
#[test]
fn a_branch_change_on_disk_is_picked_up_by_the_poll() {
    let repo = scratch("checkout");
    write_head(&repo, "ref: refs/heads/main\n");

    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_cwd("~/w");
    app.set_footer_git_cwd(&repo);
    app.draw().unwrap();
    assert!(live_text(&app).contains("~/w (main)"), "initial branch missing");

    // Nothing moved ⇒ the loop must NOT redraw (Pi repaints only inside
    // `if (this.cachedBranch !== nextBranch)`).
    assert!(!app.poll_footer_git_branch(), "an unchanged repo asked for a repaint");

    write_head(&repo, "ref: refs/heads/feature/x\n");
    assert!(app.poll_footer_git_branch(), "a real checkout did not ask for a repaint");
    app.draw().unwrap();
    let live = live_text(&app);
    assert!(live.contains("~/w (feature/x)"), "footer kept the stale branch:\n{live}");
    assert!(!live.contains("(main)"), "stale branch still rendered:\n{live}");
}
