//! The `no-ui` marker is a statement about a PROCESS, not about a session. A session id survives
//! resume (`cyrup -p ...` and then `cyrup -c`), so a marker that outlived the headless run that wrote
//! it refused every question of a later run that had a UI, and a marker left by a killed run would
//! have refused for ever. These pin the part of that which is judged in this module: whose statement
//! it is, when the writer counts as gone, and the sweep that removes what a gone writer left. The
//! parent side that writes and withdraws it is in `extension/tests/watcher.rs`, the child side in
//! `tests/unattended_parent.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_ext_subagents::background::reconcile::check_pid_liveness;

use super::*;

const HOST: &str = "test-host";
/// The pid [`by_pid`] reports alive.
const LIVE_PID: u32 = 4242;

fn alive(_: u32) -> Liveness {
    Liveness::Alive
}
fn dead(_: u32) -> Liveness {
    Liveness::Dead
}
fn inconclusive(_: u32) -> Liveness {
    Liveness::Unknown
}
fn by_pid(pid: u32) -> Liveness {
    if pid == LIVE_PID {
        Liveness::Alive
    } else {
        Liveness::Dead
    }
}
fn no_identity(_: u32) -> Option<ProcessStartIdentity> {
    None
}
fn started_at_200(_: u32) -> Option<ProcessStartIdentity> {
    Some(ProcessStartIdentity::from_token("linux:200"))
}

fn marker(pid: u32, identity: Option<&str>, hostname: &str) -> UnattendedMarker {
    UnattendedMarker {
        session_id: "root".to_owned(),
        created_at: 1,
        pid,
        process_start_identity: identity.map(ProcessStartIdentity::from_token),
        hostname: hostname.to_owned(),
    }
}

/// Write `marker` as the `no-ui` marker of `session_id`, bypassing the sweep.
fn plant(agent_dir: &Path, session_id: &str, marker: &UnattendedMarker) {
    let location = forwarding_location(agent_dir, session_id).unwrap();
    std::fs::create_dir_all(&location.session_root).unwrap();
    write_json_atomic(
        &location.session_root.join(UNATTENDED_MARKER_FILE_NAME),
        marker,
        None,
    )
    .unwrap();
}

fn marker_file(agent_dir: &Path, session_id: &str) -> PathBuf {
    forwarding_location(agent_dir, session_id)
        .unwrap()
        .session_root
        .join(UNATTENDED_MARKER_FILE_NAME)
}

fn unattended(
    agent_dir: &Path,
    liveness: fn(u32) -> Liveness,
    identity: fn(u32) -> Option<ProcessStartIdentity>,
) -> bool {
    session_is_unattended_with(agent_dir, "root", HOST, liveness, identity)
}

/// The pid of a process that has run and been reaped.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("sh")
        .args(["-c", ":"])
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn a_marker_names_the_process_that_wrote_it() {
    let dir = tempfile::tempdir().unwrap();
    assert!(mark_session_unattended(dir.path(), "root", None));

    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(marker_file(dir.path(), "root")).unwrap()).unwrap();
    assert_eq!(written["sessionId"], "root");
    assert_eq!(written["pid"], std::process::id());
    assert_eq!(written["hostname"], machine_hostname());
    assert!(
        written["processStartIdentity"].is_string(),
        "the start identity is what tells this process from a later one that gets its pid: {written}"
    );
}

#[test]
fn a_marker_stands_while_its_writer_lives_and_lapses_when_it_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    plant(dir.path(), "root", &marker(LIVE_PID, None, HOST));

    assert!(
        unattended(dir.path(), alive, no_identity),
        "a live headless root still refuses its children's questions"
    );
    assert!(
        !unattended(dir.path(), dead, no_identity),
        "a root that was killed leaves no refusal behind"
    );
}

#[test]
fn a_pid_that_another_process_has_taken_over_is_a_writer_that_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    plant(
        dir.path(),
        "root",
        &marker(LIVE_PID, Some("linux:100"), HOST),
    );

    assert!(
        !unattended(dir.path(), alive, started_at_200),
        "the pid is alive, but it started at another time: a different process"
    );
    assert!(unattended(dir.path(), alive, |_| Some(
        ProcessStartIdentity::from_token("linux:100")
    )));
    assert!(
        unattended(dir.path(), alive, no_identity),
        "a start identity that cannot be read now proves nothing"
    );
}

#[test]
fn only_positive_evidence_that_the_writer_is_gone_lifts_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    plant(dir.path(), "root", &marker(LIVE_PID, None, HOST));
    assert!(
        unattended(dir.path(), inconclusive, no_identity),
        "a probe that cannot say (EPERM under a sandbox) is not death"
    );

    plant(dir.path(), "root", &marker(LIVE_PID, None, "another-host"));
    assert!(
        unattended(dir.path(), dead, no_identity),
        "another machine's pid table says nothing about this one's"
    );
}

#[test]
fn a_marker_that_names_no_process_kill_could_mean_is_a_writer_that_is_gone() {
    // `kill(2)` reads 0 as the caller's process group and -1 (a u32::MAX pid as the signed number
    // the call takes) as every process: both answer "alive" for ever.
    let dir = tempfile::tempdir().unwrap();
    for pid in [0, u32::MAX] {
        plant(dir.path(), "root", &marker(pid, None, &machine_hostname()));
        assert!(
            !session_is_unattended(dir.path(), "root"),
            "pid {pid} is not a process that wrote this"
        );
    }
}

#[test]
fn a_marker_an_earlier_build_wrote_is_no_marker_and_is_swept_up() {
    // That build stamped no writer and never withdrew the statement: the defect.
    let dir = tempfile::tempdir().unwrap();
    let path = marker_file(dir.path(), "root");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"sessionId":"root","createdAt":1}"#).unwrap();
    assert!(!session_is_unattended(dir.path(), "root"));

    prune_unattended_markers(dir.path(), 10, HOST, alive, no_identity, None);
    assert!(!path.exists(), "removed");
    assert!(
        !path.parent().unwrap().exists(),
        "and so is the directory it left empty"
    );
}

#[test]
fn the_sweep_removes_what_a_gone_root_left_and_nothing_a_live_one_or_a_request_needs() {
    let dir = tempfile::tempdir().unwrap();
    plant(dir.path(), "gone", &marker(1, None, HOST));
    plant(dir.path(), "live", &marker(LIVE_PID, None, HOST));
    plant(dir.path(), "elsewhere", &marker(1, None, "another-host"));
    // A child's question is waiting in this root's spool: the marker goes, the request stays.
    plant(dir.path(), "waiting", &marker(1, None, HOST));
    let waiting = forwarding_location(dir.path(), "waiting").unwrap();
    std::fs::create_dir_all(&waiting.requests_dir).unwrap();
    std::fs::write(waiting.requests_dir.join("r1.json"), "{}").unwrap();
    // A directory without a marker, and a stray file, are not the sweep's.
    let pending = forwarding_location(dir.path(), "pending").unwrap();
    std::fs::create_dir_all(&pending.requests_dir).unwrap();
    let sessions = pending.session_root.parent().unwrap().to_path_buf();
    std::fs::write(sessions.join("stray"), "x").unwrap();

    prune_unattended_markers(dir.path(), 100, HOST, by_pid, no_identity, None);

    assert!(!marker_file(dir.path(), "gone").exists());
    assert!(
        !forwarding_location(dir.path(), "gone")
            .unwrap()
            .session_root
            .exists()
    );
    assert!(marker_file(dir.path(), "live").exists());
    assert!(marker_file(dir.path(), "elsewhere").exists());
    assert!(!marker_file(dir.path(), "waiting").exists());
    assert!(waiting.requests_dir.join("r1.json").exists());
    assert!(pending.requests_dir.exists());
    assert!(sessions.join("stray").exists());
}

#[test]
fn one_sweep_looks_at_a_bounded_number_of_entries() {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..12 {
        plant(dir.path(), &format!("gone-{n}"), &marker(1, None, HOST));
    }
    prune_unattended_markers(dir.path(), 5, HOST, dead, no_identity, None);

    let left = (0..12)
        .filter(|n| marker_file(dir.path(), &format!("gone-{n}")).exists())
        .count();
    assert_eq!(left, 7, "twelve markers, a bound of five: seven are left");

    prune_unattended_markers(dir.path(), 100, HOST, dead, no_identity, None);
    let left = (0..12)
        .filter(|n| marker_file(dir.path(), &format!("gone-{n}")).exists())
        .count();
    assert_eq!(left, 0, "a later sweep finishes the work");
}

#[test]
fn marking_a_session_sweeps_up_the_roots_that_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    plant(
        dir.path(),
        "killed",
        &marker(dead_pid(), None, &machine_hostname()),
    );
    plant(
        dir.path(),
        "running",
        &marker(std::process::id(), None, &machine_hostname()),
    );

    assert!(mark_session_unattended(dir.path(), "new", None));

    assert!(
        !marker_file(dir.path(), "killed").exists(),
        "the killed root's marker"
    );
    assert!(
        marker_file(dir.path(), "running").exists(),
        "a live root's is not touched"
    );
    assert!(session_is_unattended(dir.path(), "new"));
}

#[test]
fn a_marker_withdrawn_by_a_ui_is_gone_whoever_wrote_it() {
    let dir = tempfile::tempdir().unwrap();
    plant(dir.path(), "root", &marker(LIVE_PID, None, HOST));
    assert!(clear_session_unattended(dir.path(), "root", None));
    assert!(!marker_file(dir.path(), "root").exists());
    assert!(
        clear_session_unattended(dir.path(), "root", None),
        "and again"
    );
}

#[cfg(unix)]
#[test]
fn the_sweep_does_not_follow_a_symlink_out_of_the_spool() {
    let dir = tempfile::tempdir().unwrap();
    // Somewhere else holds a marker of a gone root; a link in the spool leads to it.
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    write_json_atomic(
        &elsewhere.join(UNATTENDED_MARKER_FILE_NAME),
        &marker(1, None, HOST),
        None,
    )
    .unwrap();
    let sessions = forwarding_location(dir.path(), "any")
        .unwrap()
        .session_root
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::create_dir_all(&sessions).unwrap();
    std::os::unix::fs::symlink(&elsewhere, sessions.join("link")).unwrap();

    prune_unattended_markers(dir.path(), 100, HOST, dead, no_identity, None);

    assert!(elsewhere.join(UNATTENDED_MARKER_FILE_NAME).exists());
}

#[test]
fn a_marker_that_cannot_be_removed_is_reported_as_still_there() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file belongs: `remove_file` cannot take it.
    let path = marker_file(dir.path(), "root");
    std::fs::create_dir_all(path.join("inside")).unwrap();

    assert!(!clear_session_unattended(dir.path(), "root", None));
}

#[cfg(target_os = "linux")]
#[test]
fn a_root_that_was_killed_and_not_reaped_is_a_writer_that_is_gone() {
    // A zombie still answers `kill(pid, 0)`; its parent has not collected it yet.
    let mut child = std::process::Command::new("sh")
        .args(["-c", ":"])
        .spawn()
        .unwrap();
    let pid = child.id();
    let started = std::time::Instant::now();
    while !std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z'))
    }) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the child never became a zombie"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        check_pid_liveness(pid),
        Liveness::Alive,
        "the premise: kill(pid, 0) cannot tell"
    );

    let dir = tempfile::tempdir().unwrap();
    plant(dir.path(), "root", &marker(pid, None, &machine_hostname()));
    assert!(!session_is_unattended(dir.path(), "root"));

    child.wait().unwrap();
}
