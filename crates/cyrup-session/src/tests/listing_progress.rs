//! TUI-121 — the listing's progress contract, pi's `SessionListProgress`
//! (`session-manager.ts:884-889` @v0.87.1): `(loaded, total, partialSessions?)` after every file,
//! with the partial set only on the periodic publishes (`:963-965`, `:1995-1998`), and pi's
//! `AbortSignal` as the callback's [`ControlFlow::Break`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ops::ControlFlow;
use std::path::Path;

use crate::layout::SessionsRoot;
use crate::listing::{self, SessionInfo, SessionListing};
use tempfile::TempDir;

/// `n` sessions in `dir`, file names ascending with `i`, each recorded in `cwd`.
fn write_sessions(dir: &Path, cwd: &Path, n: usize) {
    std::fs::create_dir_all(dir).unwrap();
    for i in 0..n {
        let id = format!("0193f0e1-0000-7000-8000-{i:012}");
        let header = serde_json::json!({
            "type": "session", "version": 3, "id": id,
            "timestamp": "2026-01-01T00:00:00.000Z", "cwd": cwd.display().to_string(),
        });
        let message = serde_json::json!({
            "type": "message", "id": "m1", "parentId": null,
            "timestamp": format!("2026-01-01T00:00:{:02}.000Z", i % 60),
            "message": {"role": "user", "content": format!("session {i}"), "timestamp": 1},
        });
        std::fs::write(
            dir.join(format!("2026-01-01T00-00-{i:02}-000Z_{id}.jsonl")),
            format!("{header}\n{message}\n"),
        )
        .unwrap();
    }
}

/// One `(loaded, total, partial.len())` progress report.
type Report = (usize, usize, Option<usize>);

/// Every report a listing makes, and its result.
fn reports(listing: &SessionListing) -> (Vec<Report>, Vec<SessionInfo>) {
    let mut seen = Vec::new();
    let mut cb = |loaded: usize, total: usize, partial: Option<&[SessionInfo]>| {
        seen.push((loaded, total, partial.map(<[SessionInfo]>::len)));
        ControlFlow::Continue(())
    };
    let out = listing.run(Some(&mut cb));
    (seen, out)
}

/// `listSessionsFromDir` (`:959-966`): a report per file, the partial set after the first file,
/// every tenth and the last — and the first partial is the NEWEST file (names read descending,
/// `:951`).
#[test]
fn a_directory_listing_publishes_partials_on_pis_cadence() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let dir = tmp.path().join("sessions");
    write_sessions(&dir, &cwd, 12);
    let listing = SessionListing::Dir {
        dir: dir.clone(),
        cwd_filter: None,
    };
    let (seen, out) = reports(&listing);
    assert_eq!(seen.len(), 12);
    let published: Vec<_> = seen.iter().filter(|r| r.2.is_some()).collect();
    assert_eq!(
        published,
        vec![&(1, 12, Some(1)), &(10, 12, Some(10)), &(12, 12, Some(12))]
    );
    assert_eq!(out.len(), 12);

    let mut first = None;
    let mut cb = |_: usize, _: usize, partial: Option<&[SessionInfo]>| {
        if first.is_none() {
            first = partial.map(|p| p[0].first_message.clone());
        }
        ControlFlow::Continue(())
    };
    listing.run(Some(&mut cb));
    assert_eq!(first.as_deref(), Some("session 11"));
}

/// `listAll` (`:1990-1999`): the partial set after the first candidate, every hundredth and the
/// last.
#[test]
fn the_all_projects_listing_publishes_first_and_last() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("sessions");
    write_sessions(&root.join("--a--"), &tmp.path().join("a"), 3);
    write_sessions(&root.join("--b--"), &tmp.path().join("b"), 2);
    let (seen, out) = reports(&SessionListing::AllProjects(SessionsRoot(root)));
    let published: Vec<_> = seen.iter().filter(|r| r.2.is_some()).collect();
    assert_eq!(published, vec![&(1, 5, Some(1)), &(5, 5, Some(5))]);
    assert_eq!(out.len(), 5);
}

/// A `Break` is pi's aborted signal: no further file is read.
#[test]
fn a_break_stops_the_scan() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let dir = tmp.path().join("sessions");
    write_sessions(&dir, &cwd, 5);
    let mut calls = 0;
    let mut cb = |_: usize, _: usize, _: Option<&[SessionInfo]>| {
        calls += 1;
        if calls == 2 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let out = listing::list_in_dir(&dir, None, Some(&mut cb));
    assert_eq!(calls, 2);
    assert_eq!(out.len(), 2);
}
