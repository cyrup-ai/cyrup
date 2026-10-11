//! A subagent child whose `ask` rule fires forwards the question to its root session. A root with no
//! UI (`-p`, `--mode json`) never answers: its watcher is not started and its scan returns at once
//! (pi `index.ts:1113-1116`). The child used to write its request and wait the whole forwarding
//! bound, 10 minutes, then report `User denied ...` for a question no user was asked, which is what
//! a `codemode` script that called `bash` in such a child received. The root now says it has no UI
//! ([`crate::forwarding::mark_session_unattended`]) and the child refuses at once, with the text a
//! session without a UI gives itself.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::ToolCallId;
use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension};
use serde_json::json;

use crate::forwarding::{
    CHILD_WAIT_TIMEOUT_ENV, clear_session_unattended, forwarding_location, mark_session_unattended,
    session_is_unattended,
};
use crate::{CHILD_ENV_VAR, PermissionSystemExtension};

struct Registry;
impl HostServices for Registry {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["bash".to_owned()])
    }
}

/// A subagent child over a policy that asks about every `bash` call, whose parent anchor is
/// `root_session` and whose forwarding wait is `wait_ms`. The pins are thread-local: the test runs
/// on its own current-thread runtime and holds the returned guards.
async fn child(
    agent_dir: &Path,
    cwd: &Path,
    root_session: &str,
    wait_ms: &str,
) -> (PermissionSystemExtension, Vec<crate::envx::EnvPin>) {
    std::fs::write(
        agent_dir.join("cyrup-permissions.jsonc"),
        json!({ "tools": { "bash": "ask" } }).to_string(),
    )
    .unwrap();
    let pins = vec![
        crate::envx::pin(CHILD_ENV_VAR, Some("1")),
        crate::envx::pin(
            cyrup_ext_subagents::PARENT_SESSION_ENV_VAR,
            Some(root_session),
        ),
        crate::envx::pin(CHILD_WAIT_TIMEOUT_ENV, Some(wait_ms)),
    ];
    let ext =
        PermissionSystemExtension::new_forwarding_child(agent_dir.to_path_buf(), cwd.to_path_buf());
    ext.set_host_services(Arc::new(Registry));
    ext.init(&mut InitApi::new()).await.unwrap();
    (ext, pins)
}

async fn ask_bash(ext: &PermissionSystemExtension, cwd: &Path) -> (HookOutcome, Duration) {
    let started = Instant::now();
    let outcome = ext
        .on_event(
            &HostEvent::ToolCall {
                call_id: ToolCallId::from("c1"),
                name: "bash".to_owned(),
                input: json!({ "command": "ls" }),
            },
            &HostCtx::event(ExtMode::Print, false, cwd.to_path_buf()),
        )
        .await;
    (outcome, started.elapsed())
}

fn reason(outcome: &HookOutcome) -> String {
    match outcome {
        HookOutcome::Block { reason, .. } => reason.clone().unwrap_or_default(),
        other => panic!("expected a block, got {other:?}"),
    }
}

#[tokio::test]
async fn a_child_whose_root_has_no_ui_is_refused_at_once_and_not_after_the_forwarding_wait() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    assert!(mark_session_unattended(&agent_dir, "root-headless", None));

    // Five seconds is the wait the child would sit out; it must not.
    let (ext, _pins) = child(&agent_dir, dir.path(), "root-headless", "5000").await;
    let (outcome, took) = ask_bash(&ext, dir.path()).await;

    assert!(
        took < Duration::from_secs(2),
        "waited {took:?} for a root with no UI"
    );
    let reason = reason(&outcome);
    assert!(
        reason.contains("requires approval, but no interactive UI is available"),
        "{reason}"
    );
    assert!(
        !reason.contains("User denied"),
        "nobody was asked: {reason}"
    );
    // And nothing was left in the root's spool for it to find later.
    let location = forwarding_location(&agent_dir, "root-headless").unwrap();
    assert!(!location.requests_dir.exists(), "no request was written");
}

#[tokio::test]
async fn a_child_whose_root_has_not_said_it_has_no_ui_still_forwards_and_waits() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // Another session's marker says nothing about this root.
    assert!(mark_session_unattended(&agent_dir, "some-other-root", None));

    let (ext, _pins) = child(&agent_dir, dir.path(), "root-attended", "300").await;
    let (outcome, took) = ask_bash(&ext, dir.path()).await;

    assert!(
        took >= Duration::from_millis(250),
        "forwarded and waited: {took:?}"
    );
    assert!(reason(&outcome).contains("User denied"), "{outcome:?}");
}

/// The marker a headless root left and then died with (killed, crashed, or simply finished while a
/// background child went on): it names a process that is no longer there.
fn plant_marker_of_a_dead_root(agent_dir: &Path, session: &str) {
    let mut gone = std::process::Command::new("sh")
        .args(["-c", ":"])
        .spawn()
        .unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    let location = forwarding_location(agent_dir, session).unwrap();
    std::fs::create_dir_all(&location.session_root).unwrap();
    std::fs::write(
        location.session_root.join("no-ui"),
        json!({
            "sessionId": session,
            "createdAt": 1,
            "pid": pid,
            "hostname": cyrup_ext_subagents::background::async_retention::machine_hostname(),
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn a_child_whose_root_died_with_its_marker_forwards_and_waits_like_any_other() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    plant_marker_of_a_dead_root(&agent_dir, "root-killed");
    // The statement was true of a process that is gone, and the session may be resumed with a UI.
    assert!(!session_is_unattended(&agent_dir, "root-killed"));

    let (ext, _pins) = child(&agent_dir, dir.path(), "root-killed", "300").await;
    let (outcome, took) = ask_bash(&ext, dir.path()).await;

    assert!(
        took >= Duration::from_millis(250),
        "forwarded and waited: {took:?}"
    );
    assert!(reason(&outcome).contains("User denied"), "{outcome:?}");
}

#[test]
fn a_marker_is_per_session_and_can_be_withdrawn() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!session_is_unattended(dir.path(), "root-1"));
    assert!(mark_session_unattended(dir.path(), "root-1", None));
    assert!(session_is_unattended(dir.path(), "root-1"));
    assert!(!session_is_unattended(dir.path(), "root-2"));

    // The marker is private to the user, like the rest of the spool.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let location = forwarding_location(dir.path(), "root-1").unwrap();
        let mode = std::fs::metadata(location.session_root.join("no-ui"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    assert!(clear_session_unattended(dir.path(), "root-1", None));
    assert!(!session_is_unattended(dir.path(), "root-1"));
    // Withdrawing it again, or for a session that never had one, is not an error.
    assert!(clear_session_unattended(dir.path(), "root-1", None));
    assert!(clear_session_unattended(dir.path(), "never-marked", None));

    // An id that names no session, or that cannot be a path, has no marker.
    assert!(!mark_session_unattended(dir.path(), "unknown", None));
    assert!(!mark_session_unattended(dir.path(), "", None));
    assert!(!session_is_unattended(dir.path(), "unknown"));
}
