//! DRIFT-004 — a WASM guest supplies the bash BACKEND a `user_bash` command runs through, live.
//!
//! Upstream this is one field on one result: a `user_bash` handler returns
//! `{ operations }` and `executeBash` resolves `options?.operations ??
//! createLocalBashOperations({ shellPath })` (`packages/coding-agent/src/core/agent-session.ts:2782`
//! @v0.84.4), which is how `examples/extensions/ssh.ts:203-206`, `sandbox/index.ts:229-231` and
//! `gondolin/index.ts:517-520` each redirect one command to a remote or sandboxed shell without
//! re-implementing the bash seam. cyrup could not express it for a guest at all: a
//! `cyrup_tools::ops::BashOperations` is a callable and ADR-0002 makes extension I/O values, so the
//! `operations` KEY crossed and nothing could be behind it.
//!
//! The round trip that closes it — `registration.register-bash-operations` (declare),
//! `events.bash-operations-exec` (run), `host-bash.emit-bash-output` (pi's `onData`) and
//! `host-bash.is-bash-cancelled` (pi's `signal`) — can only be proven with a real component in a
//! real store, which is why this test is here and not in `cyrup-ext`'s in-process suite. It asks
//! the HOST for the backend the way `AgentSession::execute_bash_with_user_event` does, then drives
//! it exactly as `run_bash` does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::fixture;

use cyrup_core::CancelToken;
use cyrup_ext::DenyServices;
use cyrup_tools::ops::{BashExecOptions, ExitStatus};
use std::sync::Arc;

/// Load the demo guest and hand back the host that owns it.
async fn host_with_demo() -> cyrup_ext::ExtensionHost {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init");
    host
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_supplied_bash_backend_runs_the_command_and_streams_its_output() {
    let host = host_with_demo().await;

    // The guest declared a backend during `init` (`registration.register-bash-operations`), so the
    // host can resolve one for the extension whose `user_bash` result won — upstream reads
    // `operations` off exactly that one result (`extensions/runner.ts:1005-1032`).
    let ops = host
        .user_bash_operations(&"demo".into(), "remote: uname -a", false, "/work")
        .expect(
            "the host must resolve a GUEST-supplied backend; before DRIFT-004 this was always \
             `None` for a wasm extension and the command silently ran on the LOCAL shell — the \
             failure mode ADR-0002's rejected-alternative D names",
        );

    // Drive it the way `cyrup_session_svc::run_bash` drives the `options?.operations` branch.
    let mut streamed: Vec<u8> = Vec::new();
    let mut sink = |data: &[u8]| streamed.extend_from_slice(data);
    let status = ops
        .exec(
            "remote: uname -a",
            std::path::Path::new("/work"),
            BashExecOptions {
                on_data: &mut sink,
                cancel: CancelToken::new(),
                timeout: None,
                env: vec![("PI_MODEL".into(), "opus".into())],
                env_remove: Vec::new(),
            },
        )
        .await
        .expect("the guest backend ran the command");

    assert_eq!(
        status,
        ExitStatus::Exited(0),
        "pi's `{{ exitCode: 0 }}` came back across the export"
    );
    let out = String::from_utf8_lossy(&streamed);
    assert!(
        out.contains("[demo-backend] remote: uname -a in /work"),
        "the output must be the GUEST's, streamed over `host-bash.emit-bash-output` (pi's \
         `onData`) — got {out:?}"
    );
    assert!(
        out.contains("env=1 timeout=None"),
        "the VALUE half of pi's options bag (`timeout?`, `env?`, `core/tools/bash.ts:77-78`) must \
         reach the backend too — got {out:?}"
    );
}

/// pi's `throw`: a backend FAILURE is re-raised past the abort check (`core/bash-executor.ts:154`)
/// and must never be reported as a command that ran and produced nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_guest_backend_is_an_error_not_an_empty_success() {
    let host = host_with_demo().await;
    let ops = host
        .user_bash_operations(&"demo".into(), "boom", false, "/work")
        .expect("backend resolved");

    let mut sink = |_: &[u8]| {};
    let err = ops
        .exec(
            "boom",
            std::path::Path::new("/work"),
            BashExecOptions {
                on_data: &mut sink,
                cancel: CancelToken::new(),
                timeout: None,
                env: Vec::new(),
                env_remove: Vec::new(),
            },
        )
        .await
        .expect_err("the guest's `Err` is pi's `throw`");
    assert!(
        err.to_string().contains("demo backend refused to run"),
        "the guest's own message must survive: {err}"
    );
}

/// pi's `signal`: an already-aborted command reports `exitCode: null` — cyrup's
/// [`ExitStatus::Killed`], which is what keeps a cancel distinguishable from a timeout and from an
/// external signal instead of collapsing into upstream's single `null`. The demo backend polls
/// `host-bash.is-bash-cancelled` (pi's `signal.aborted`) before it does anything, exactly as
/// `createLocalShellOperations` does (`core/tools/bash.ts:88-90`: `if (signal?.aborted) throw new Error("aborted")`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_aborted_command_is_killed_not_run() {
    let host = host_with_demo().await;
    let ops = host
        .user_bash_operations(&"demo".into(), "remote: sleep 100", false, "/work")
        .expect("backend resolved");

    let cancel = CancelToken::new();
    cancel.cancel();
    let mut streamed: Vec<u8> = Vec::new();
    let mut sink = |data: &[u8]| streamed.extend_from_slice(data);
    let status = ops
        .exec(
            "remote: sleep 100",
            std::path::Path::new("/work"),
            BashExecOptions {
                on_data: &mut sink,
                cancel,
                timeout: None,
                env: Vec::new(),
                env_remove: Vec::new(),
            },
        )
        .await
        .expect("a cancelled command is not a backend failure");
    assert_eq!(status, ExitStatus::Killed);
    assert!(
        streamed.is_empty(),
        "nothing ran, so nothing streamed: {streamed:?}"
    );
}

/// An extension that never declared a backend resolves to `None` — upstream's ABSENT `operations`,
/// which falls through to `createLocalBashOperations` (`core/agent-session.ts:2782`'s `??`). Pinned
/// against the same live host, so the positive cases above cannot be passing for a reason that
/// would make every lookup succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_that_declared_no_backend_still_falls_through_to_the_local_shell() {
    let host = host_with_demo().await;
    assert!(
        host.user_bash_operations(&"not-loaded".into(), "uname -a", false, "/work")
            .is_none()
    );
}
