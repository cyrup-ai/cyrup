//! The writer — [`update_terminal_run_index`].
//!
//! Ports pi `updateTerminalRunIndex` (`terminal-run-index.ts:50-61`).

use std::path::Path;

use crate::background::atomic::write_atomic_json_creating_parent;
use crate::background::{RunId, RunStatus};

use super::entry::{TerminalIndexVersion, TerminalRunIndexEntry, is_indexed_state, marker_path};

/// Record `status` in the terminal-run index — pi `updateTerminalRunIndex` (`:50-61`).
///
/// A no-op unless the run is BOTH in an indexed state AND session-attributed (`:51`). The session
/// guard is the invariant this module exists for: the index's only purpose is to answer "what
/// finished in MY session", and an unattributed marker is not merely useless, it is a row every
/// reader must then re-filter. `Option<SessionId>` makes the guard a `let else` rather than pi's
/// two-clause `typeof` test — [`crate::identity::SessionId`] cannot be empty by construction.
///
/// # Errors
///
/// A directory-creation or write failure. Every caller treats this as best-effort (pi's own
/// caller wraps it in `try`/`catch`, `active-run-index.ts:100-108`) — an advisory index that
/// failed to write must never fail the run that was finishing.
pub async fn update_terminal_run_index(
    async_dir: &Path,
    status: &RunStatus,
) -> std::io::Result<()> {
    if !is_indexed_state(status.state) {
        return Ok(());
    }
    let Some(session_id) = status.session_id.clone() else {
        return Ok(());
    };

    // pi `status.endedAt ?? status.lastUpdate ?? status.startedAt` (`:52`). cyrup's `last_update`
    // is a non-optional `i64` (`background/records.rs`), so the third rung is unreachable and the
    // chain collapses to two.
    let ended_at = status.ended_at.unwrap_or(status.last_update);

    let Some(marker) = marker_path(async_dir, &session_id, ended_at) else {
        return Ok(());
    };

    // pi `status.runId || path.basename(asyncDir)` (`:55`). The entry's run id and the marker
    // NAME's directory component can therefore disagree; the reader resolves the run by the
    // ENTRY's id and re-verifies against the live status (see `super::read`).
    let run_id = if status.run_id.as_str().is_empty() {
        RunId::from_token(
            async_dir
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or_default(),
        )
    } else {
        status.run_id.clone()
    };

    write_atomic_json_creating_parent(
        &marker,
        &TerminalRunIndexEntry {
            version: TerminalIndexVersion,
            run_id,
            session_id,
            ended_at,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::{RunMode, RunState};
    use crate::identity::SessionId;

    fn status(state: RunState, session: Option<&str>) -> RunStatus {
        let mut status = RunStatus::queued(RunId::from_token("run1"), RunMode::Single, Some(1));
        status.state = state;
        status.session_id = SessionId::parse_opt(session);
        status.last_update = 42;
        status
    }

    fn markers_under(root: &Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let Ok(sessions) = std::fs::read_dir(super::super::entry::index_root(root)) else {
            return found;
        };
        for session in sessions.flatten() {
            if let Ok(files) = std::fs::read_dir(session.path()) {
                for file in files.flatten() {
                    found.push(file.path());
                }
            }
        }
        found.sort();
        found
    }

    #[tokio::test]
    async fn an_active_run_writes_no_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        for state in [RunState::Queued, RunState::Running] {
            update_terminal_run_index(&async_dir, &status(state, Some("s1")))
                .await
                .expect("no-op is ok");
        }
        assert!(markers_under(tmp.path()).is_empty());
    }

    #[tokio::test]
    async fn an_unattributed_run_writes_no_marker() {
        // pi `:51` — the session guard: an unattributed marker could never answer "what finished
        // in MY session" and is refused at the write.
        let tmp = tempfile::tempdir().expect("tempdir");
        update_terminal_run_index(&tmp.path().join("run1"), &status(RunState::Complete, None))
            .await
            .expect("no-op is ok");
        assert!(markers_under(tmp.path()).is_empty());
    }

    #[tokio::test]
    async fn every_indexed_state_including_paused_writes_a_marker() {
        // §1.1 — `Paused` is IN the indexed set here, unlike `RunState::is_terminal`.
        for state in [
            RunState::Paused,
            RunState::Complete,
            RunState::Failed,
            RunState::Stopped,
        ] {
            let tmp = tempfile::tempdir().expect("tempdir");
            update_terminal_run_index(&tmp.path().join("run1"), &status(state, Some("s1")))
                .await
                .expect("write succeeds");
            assert_eq!(markers_under(tmp.path()).len(), 1, "state {state:?}");
        }
    }

    #[tokio::test]
    async fn the_marker_records_the_status_identity_and_falls_back_to_last_update() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut s = status(RunState::Complete, Some("s1"));
        s.ended_at = None; // force the `?? lastUpdate` rung (pi `:52`)
        update_terminal_run_index(&tmp.path().join("run1"), &s)
            .await
            .expect("write");

        let markers = markers_under(tmp.path());
        assert_eq!(markers.len(), 1);
        let name = markers[0].file_name().unwrap().to_str().unwrap();
        assert_eq!(
            name, "0000000000000042-run1.json",
            "endedAt fell back to lastUpdate"
        );
        let entry = TerminalRunIndexEntry::parse(&std::fs::read(&markers[0]).expect("read"))
            .expect("parses");
        assert_eq!(entry.run_id.as_str(), "run1");
        assert_eq!(entry.session_id.as_str(), "s1");
        assert_eq!(entry.ended_at, 42);
    }

    #[tokio::test]
    async fn an_empty_status_run_id_falls_back_to_the_directory_basename() {
        // pi `status.runId || path.basename(asyncDir)` (`:55`).
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut s = status(RunState::Complete, Some("s1"));
        s.run_id = RunId::from_token("");
        s.ended_at = Some(7);
        update_terminal_run_index(&tmp.path().join("dirname1"), &s)
            .await
            .expect("write");

        let markers = markers_under(tmp.path());
        assert_eq!(markers.len(), 1);
        let entry = TerminalRunIndexEntry::parse(&std::fs::read(&markers[0]).expect("read"))
            .expect("parses");
        assert_eq!(entry.run_id.as_str(), "dirname1");
    }

    #[tokio::test]
    async fn ended_at_wins_over_last_update_when_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut s = status(RunState::Complete, Some("s1"));
        s.ended_at = Some(99);
        update_terminal_run_index(&tmp.path().join("run1"), &s)
            .await
            .expect("write");
        let markers = markers_under(tmp.path());
        let name = markers[0].file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("0000000000000099-"), "got {name}");
    }
}
