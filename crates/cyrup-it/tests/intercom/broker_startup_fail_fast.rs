//! Regression proof for the config.rs dossier item ("Invalid ask-timeout env var: pi crash-fails
//! before any startup side effect, cyrup silently defaults after the socket/pid are already live"):
//! pi's `getAskTimeoutMs()` throws inside `new IntercomBroker()` — a class-field initializer that
//! runs INSIDE the constructor, before `.start()` ever binds the listener or writes any file
//! (`broker.ts:139`). A malformed `PI_INTERCOM_ASK_TIMEOUT_MS` therefore crashes the process before
//! ANY socket/pid file exists.
//!
//! Before the fix, `broker::run()` called `config::ask_timeout_ms()` AFTER `UnixListener::bind`,
//! `restrict_intercom_runtime_file`, and `std::fs::write(pid_path)` had already succeeded — so an
//! external process polling for broker readiness (socket connectable + pid file present) could
//! observe a fully "started" broker for a brief window before it exited with an error, exactly
//! backwards from pi's fail-before-any-side-effect guarantee. This test launches the REAL broker
//! subprocess (the same `cyrup-intercom-broker` fixture binary `broker_roundtrip.rs` uses) with an
//! invalid ask-timeout env var and asserts NEITHER the socket NOR the pid file is ever created.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_ask_timeout_env_fails_before_any_socket_or_pid_file_exists() {
    let broker_bin = crate::support::bins::intercom_broker();
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = agent_dir.path().join("intercom");
    let socket_path = intercom_dir.join("broker.sock");
    let pid_path = intercom_dir.join("broker.pid");

    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        // Not a positive integer → `config::ask_timeout_ms()` must hard-`Err` (config.ts:14-16).
        .env("CYRUP_INTERCOM_ASK_TIMEOUT_MS", "not-a-number")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the real intercom broker subprocess");

    let status = tokio::time::timeout(Duration::from_secs(5), broker.wait())
        .await
        .expect("the broker must exit promptly on an invalid ask-timeout env var, not hang")
        .expect("wait succeeds");

    assert!(
        !status.success(),
        "an invalid ask-timeout env var must be a hard startup failure"
    );
    assert!(
        !socket_path.exists(),
        "the socket must never be bound before the ask-timeout env var is validated"
    );
    assert!(
        !pid_path.exists(),
        "the pid file must never be written before the ask-timeout env var is validated"
    );
}
