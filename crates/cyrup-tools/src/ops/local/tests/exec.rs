//! [`LocalProc::exec`] — the `bash`-tool / immediate-bash path, whose every termination leg is an
//! immediate `killpg` of the `setsid`'d shell's whole process group.
//!
//! [`LocalProc::exec`]: crate::ops::ProcOps::exec
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use cyrup_core::CancelToken;

/// THE JS→Rust mechanism gap [`KillTreeOnDrop`] closes: pi's abort/timeout handling hangs off an
/// `async` function that always settles (`bash.ts:111-121` → `killProcessTree`,
/// `shell.ts:200-225`), so the shell's process GROUP can never outlive the call. A Rust future
/// can be dropped at any `.await` — here by `tokio::time::timeout`, but equally by a cancelled
/// `tokio::spawn`, an unwinding panic, or runtime teardown — and every `send_sigkill_tree` arm
/// in `exec`'s `select!` is then simply never reached.
///
/// RED before the guard: `kill_on_drop(true)` SIGKILLs the direct `setsid` shell ONLY, so the
/// backgrounded `sleep 30` in its process group survives the drop for its full 30s (recorded as
/// an unfixed consequence in `12-upstream-drift-pi-core.md`'s `DRIFT-043` rejection note —
/// "grandchildren do survive — single-pid kill, not killpg"). GREEN after: the group is
/// `killpg`'d on the drop path exactly as it is on every non-drop path.
///
/// [`KillTreeOnDrop`]: crate::ops::local::guard::KillTreeOnDrop
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_exec_future_kills_the_whole_process_group_not_just_the_direct_child() {
    let proc = LocalProc::new(ShellConfig::detect());
    let marker =
        std::env::temp_dir().join(format!("cyrup-exec-dropguard-{}.pid", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    // The shell backgrounds a descendant in its own (`setsid`) process group and then blocks, so
    // the future is still mid-`select!` when the timeout below drops it.
    let spec = exec_spec(&format!("sleep 30 & echo $! > {}; wait", marker.display()));

    let elapsed = tokio::time::timeout(
        Duration::from_millis(500),
        proc.exec(spec, CancelToken::new(), None, &mut |_data: &[u8]| {}),
    )
    .await;
    assert!(
        elapsed.is_err(),
        "fixture: the command must still be running when the timeout DROPS the future — \
         otherwise this test observes a normal return, not the drop path"
    );

    let descendant: u32 = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .expect("fixture: the backgrounded descendant must have recorded its pid");
    let _ = std::fs::remove_file(&marker);

    // A `killpg`'d process is a zombie until its (now-dead) parent's reaper collects it, and
    // `kill(pid, 0)` succeeds on a zombie, so poll rather than sampling once. A survivor would
    // stay observable for the full 30s, so this bound discriminates by a wide margin.
    let mut gone = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if !pid_exists(descendant) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Cleanup BEFORE the assertion so a failing run does not itself leak the `sleep 30` it is
    // complaining about.
    let _ = kill_pid(descendant);
    assert!(
        gone,
        "dropping the `exec` future must `killpg` the whole `setsid` group — the backgrounded \
         descendant (pid {descendant}) outlived the drop, which is `kill_on_drop`'s single-pid \
         behaviour, not pi's `killProcessTree`"
    );
}

/// `LocalProc::exec` (the `bash` tool / immediate-bash backend) must SIGKILL a SIGTERM-ignoring
/// tree IMMEDIATELY on timeout — Pi's real `killProcessTree` (`shell.ts:200-225`), called
/// directly by `bash.ts:118-121`'s timeout handler with NO intervening `SIGTERM`/grace step.
/// Configuring an intentionally huge `kill_grace` (5s, the SAME value `exec_argv` actually
/// waits out) and still finishing in well under a second proves `exec` never consults
/// `kill_grace` at all — the exact regression this fix closes (it previously reused
/// `exec_argv`'s `SIGTERM`-then-5s-grace-then-`SIGKILL` escalation, giving a SIGTERM-ignoring
/// child up to 5s of extra unsupervised runtime Pi's bash tool never grants).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_timeout_sigkills_a_sigterm_ignoring_child_immediately_no_grace() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
    let started = tokio::time::Instant::now();
    let status = proc
        .exec(
            exec_spec("trap '' TERM; while true; do sleep 1; done"),
            CancelToken::new(),
            Some(Duration::from_millis(100)),
            &mut |_data: &[u8]| {},
        )
        .await
        .expect("exec runs");
    assert_eq!(
        status,
        ExitStatus::TimedOut,
        "the timeout reason is reported even though the tree never gracefully exited"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a SIGTERM-ignoring tree must still die within ~100ms of the timeout via immediate \
         SIGKILL — no 5s grace wait like `exec_argv`'s `killProcess` escalation — got {:?}",
        started.elapsed()
    );
}

/// The same immediate-SIGKILL behavior on the `cancel` path (Pi `bash.ts:111-113`'s `onAbort`,
/// which also calls `killProcessTree` directly with no grace step).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_cancel_sigkills_a_sigterm_ignoring_child_immediately_no_grace() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
    let cancel = CancelToken::new();
    let started = tokio::time::Instant::now();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let spec = exec_spec("trap '' TERM; while true; do sleep 1; done");
        async move { proc.exec(spec, cancel, None, &mut |_data: &[u8]| {}).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    let status = task.await.expect("task joins").expect("exec runs");
    assert_eq!(status, ExitStatus::Killed, "the cancel reason is reported");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a SIGTERM-ignoring tree must still die within ~100ms of cancel via immediate SIGKILL, \
         got {:?}",
        started.elapsed()
    );
}

/// An ALREADY-cancelled token must never spawn a process at all — Pi's real
/// `createLocalBashOperations.exec` checks `signal?.aborted` and throws BEFORE calling `spawn()`
/// (`bash.ts:86-88`), ahead of even the cwd-exists check.
///
/// TOOL-030: proven WITHOUT any wall-clock bound. The cwd is deliberately a path that does not
/// exist, which makes the short-circuit's position observable rather than merely fast: the
/// cancel check at `LocalProc::exec` sits strictly BEFORE the `Working directory does not
/// exist` guard, which itself sits before `spawn()`. So
///   * short-circuit present ⇒ `Ok(ExitStatus::Killed)` (this assertion),
///   * short-circuit removed ⇒ `Err("Working directory does not exist: …")`, and
///   * short-circuit moved after `spawn()` ⇒ still `Err`, since the spawn itself fails.
/// No ordering other than Pi's can produce `Ok(Killed)` here. The marker check is kept as a
/// belt-and-braces witness (it is NOT sufficient on its own — a child that spawned and was
/// killed before `touch` completed also leaves it absent).
#[tokio::test]
async fn exec_pre_cancelled_never_spawns() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
    let marker = std::env::temp_dir().join(format!("cyrup-exec-precancel-{}", unique_suffix()));
    let missing_cwd =
        std::env::temp_dir().join(format!("cyrup-exec-precancel-cwd-{}", unique_suffix()));
    assert!(!missing_cwd.exists(), "the sentinel cwd must not exist");
    let cancel = CancelToken::new();
    cancel.cancel();
    let spec = ExecSpec {
        cwd: missing_cwd,
        ..exec_spec(&format!("touch {}", marker.display()))
    };
    let status = proc
        .exec(spec, cancel, None, &mut |_data: &[u8]| {})
        .await
        .expect(
            "a pre-cancelled exec resolves Ok(Killed) — reaching the cwd-exists guard or \
             `spawn()` at all would have produced Err",
        );
    assert_eq!(
        status,
        ExitStatus::Killed,
        "pre-cancelled reports the same reason as mid-run cancel"
    );
    assert!(
        !marker.exists(),
        "the shell command must NEVER have run — an already-cancelled token guarantees zero \
         process execution, matching Pi's pre-spawn `signal?.aborted` check"
    );
}
