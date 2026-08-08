//! G144 — the broker's runtime claim, proved against TWO REAL broker processes sharing one runtime
//! directory (`assertNoLiveBroker`, **v0.9.2** `pi-intercom/broker/runtime-claim.ts:3-21`, called
//! from the `IntercomBroker` constructor at **v0.9.2** `broker/broker.ts:231`).
//!
//! Before the fix, `broker::run` unconditionally `remove_file`d `broker.sock` and bound its own
//! (`broker/mod.rs`, the `// Unlink a stale socket left by a crashed broker` line). A second broker
//! launched against a live incumbent therefore SUCCEEDED, stealing the socket name. That is not a
//! clean takeover: the incumbent still owns the unlinked inode and keeps serving every connection it
//! had already accepted, while every new client reaches the usurper — a silent split-brain in which
//! two disjoint sets of sessions each believe they can see "all" sessions.
//!
//! The two halves of the contract pull in opposite directions and BOTH are tested here, because
//! over-fixing is as damaging as not fixing: refuse on a live incumbent (`live_incumbent_is_...`),
//! but still reclaim a runtime dir whose broker was SIGKILLed (`stale_socket_and_pid_are_...`) — a
//! presence check on `broker.pid` would deadlock intercom until a human deleted the file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::protocol::{SessionRegistration, now_ms};
use cyrup_intercom::transport::spawn::wait_for_broker;

fn broker_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"))
}

fn registration(name: &str) -> SessionRegistration {
    SessionRegistration {
        name: Some(name.to_string()),
        cwd: "/tmp/work".to_string(),
        model: "test-model".to_string(),
        pid: std::process::id().into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        status: None,
        extra: Default::default(),
    }
}

/// A long-lived broker: stdio discarded, so a pipe nobody drains can never stall it.
fn spawn_broker(agent_dir: &Path) -> tokio::process::Child {
    tokio::process::Command::new(broker_bin())
        .env("CYRUP_CODING_AGENT_DIR", agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess")
}

/// A broker we expect to exit immediately, with its stderr captured so the refusal can be asserted.
fn spawn_broker_capturing(agent_dir: &Path) -> tokio::process::Child {
    tokio::process::Command::new(broker_bin())
        .env("CYRUP_CODING_AGENT_DIR", agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the real intercom broker subprocess")
}

/// A second broker must DECLINE while the first is alive — and, critically, the first must still be
/// serving its already-registered clients afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_incumbent_broker_is_not_replaced_and_keeps_serving_its_clients() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = agent_dir.path().join("intercom");
    let socket_path = intercom_dir.join("broker.sock");
    let pid_path = intercom_dir.join("broker.pid");

    // --- The incumbent, with a client already attached. ---
    let mut incumbent = spawn_broker(agent_dir.path());
    wait_for_broker(&socket_path, Duration::from_secs(5)).await.expect("incumbent broker up");
    assert!(pid_path.exists(), "the incumbent published its pid file");
    let incumbent_pid = std::fs::read_to_string(&pid_path).expect("pid file").trim().to_string();

    let client = IntercomClient::connect(&socket_path, registration("early"), Some("early-session".to_string()))
        .await
        .expect("the early client registers with the incumbent");

    // --- The usurper: same runtime dir, live incumbent. ---
    let usurper = spawn_broker_capturing(agent_dir.path());
    let output = tokio::time::timeout(Duration::from_secs(5), usurper.wait_with_output())
        .await
        .expect("the second broker must exit promptly, not sit there having stolen the socket")
        .expect("wait succeeds");

    assert!(
        !output.status.success(),
        "a second broker must refuse to replace a live incumbent (runtime-claim.ts:20)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Refusing to replace live intercom broker process"),
        "the refusal must name upstream's reason; stderr was: {stderr}"
    );
    assert!(stderr.contains(&incumbent_pid), "the refusal names the incumbent's pid; stderr: {stderr}");

    // --- The incumbent's runtime is untouched: socket + pid file still its own. ---
    assert!(socket_path.exists(), "the refused broker must not have unlinked the incumbent's socket");
    assert_eq!(
        std::fs::read_to_string(&pid_path).expect("pid file").trim(),
        incumbent_pid,
        "the refused broker must not have overwritten the incumbent's pid file"
    );

    // --- THE POINT: the already-attached client is still served, over the same socket. ---
    let sessions = client.list_sessions().await.expect("the early client's broker still answers");
    assert!(
        sessions.iter().any(|s| s.id == "early-session"),
        "the incumbent still knows its registered session: {sessions:?}"
    );

    // --- And a NEW client reaching that socket name lands on the SAME broker, not a usurper. ---
    let late = IntercomClient::connect(&socket_path, registration("late"), Some("late-session".to_string()))
        .await
        .expect("a new client registers with the surviving incumbent");
    let sessions = late.list_sessions().await.expect("list");
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"early-session") && ids.contains(&"late-session"),
        "one broker, one session graph — a stolen socket would have split these: {ids:?}"
    );

    client.disconnect();
    late.disconnect();
    let _ = incumbent.kill().await;
}

/// The failure the claim must NOT introduce: a SIGKILLed broker leaves BOTH `broker.sock` and
/// `broker.pid` behind (it never reaches `shutdown_broker`), and the next broker must reclaim them.
/// A pid-file *presence* check would wedge intercom here until a human deleted the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_socket_and_pid_file_are_reclaimed_by_the_next_broker() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = agent_dir.path().join("intercom");
    let socket_path = intercom_dir.join("broker.sock");
    let pid_path = intercom_dir.join("broker.pid");

    let mut crashed = spawn_broker(agent_dir.path());
    wait_for_broker(&socket_path, Duration::from_secs(5)).await.expect("first broker up");
    let dead_pid = std::fs::read_to_string(&pid_path).expect("pid file").trim().to_string();

    // SIGKILL: no shutdown handler runs, so the socket and pid file survive their owner.
    // `kill()` also reaps the child, so its pid is genuinely gone (a zombie would still answer
    // `kill(pid, 0)`, which is precisely what makes reaping part of this precondition).
    crashed.kill().await.expect("kill the first broker");
    assert!(socket_path.exists(), "precondition: a SIGKILLed broker leaves its socket behind");
    assert!(pid_path.exists(), "precondition: a SIGKILLed broker leaves its pid file behind");

    // The successor must start anyway.
    let mut successor = spawn_broker(agent_dir.path());
    wait_for_broker(&socket_path, Duration::from_secs(5))
        .await
        .expect("a stale socket + pid file must be reclaimable, or a crash wedges intercom forever");

    assert!(
        successor.try_wait().expect("try_wait").is_none(),
        "the successor is still running — it reclaimed the runtime rather than refusing"
    );
    assert_ne!(
        std::fs::read_to_string(&pid_path).expect("pid file").trim(),
        dead_pid,
        "the successor published its own pid over the stale one"
    );

    let client = IntercomClient::connect(&socket_path, registration("after"), Some("after-session".to_string()))
        .await
        .expect("the reclaimed socket serves new clients");
    let sessions = client.list_sessions().await.expect("list");
    assert!(sessions.iter().any(|s| s.id == "after-session"), "{sessions:?}");

    client.disconnect();
    let _ = successor.kill().await;
}
