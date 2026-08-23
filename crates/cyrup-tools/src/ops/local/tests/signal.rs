//! The single-pid signal primitives in [`crate::ops::local::signal`].
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

/// `terminate_pid`'s `bool` return is the signal callers (`cyrup_ext::caps::proc::ProcCaps::kill`)
/// rely on to decide whether to wait out a grace period at all — `Ok(true)` on unix means a REAL
/// `SIGTERM` was sent (so waiting for a reaction is meaningful), verified here by actually
/// terminating a real spawned child and confirming it dies within the standard grace window.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminate_pid_reports_true_and_the_real_process_dies() {
    let mut child = tokio::process::Command::new("sleep")
        .arg("30")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("sleep spawns");
    let pid = child.id().expect("spawned child has a pid");

    let sent = terminate_pid(pid).expect("SIGTERM send succeeds");
    assert!(
        sent,
        "unix terminate_pid must report a real signal was sent"
    );

    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("the SIGTERM-obeying child dies within the grace window")
        .expect("wait succeeds");
    assert!(
        !status.success(),
        "a SIGTERM-terminated child does not exit successfully"
    );
}
