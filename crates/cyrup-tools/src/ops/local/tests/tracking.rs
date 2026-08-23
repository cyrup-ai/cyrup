//! The process-global detached-child registry and its drain
//! ([`crate::ops::local::tracking`]), plus the [`crate::ops::local::guard::KillTreeOnDrop`] half
//! that un-enrols a pid on an abandoned future.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::ops::Transport;
use crate::ops::local::command::build_command;
use crate::ops::local::tracking::drain_and_kill;
use cyrup_core::CancelToken;
use tokio::io::AsyncWriteExt;

/// SEAM-S03, the registry half: `LocalProc::exec` must enroll its `setsid` shell for the whole
/// time that shell is running and remove it when the exec ends — Pi's
/// `if (child.pid) trackDetachedChildPid(child.pid);` at the spawn (`core/tools/bash.ts:108`
/// @v0.83.0) and the matching `untrackDetachedChildPid` in that spawn's `finally` (`:142`).
///
/// The membership is asserted PRESENT first, from inside the `on_data` callback while the child
/// is provably alive (it has just written its own `$$` and is now blocked in `sleep`). Without
/// that half the absence assertion afterwards would pass just as well against a registry that
/// was never written to at all.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_tracks_its_detached_shell_for_exactly_as_long_as_it_runs() {
    let proc = LocalProc::new(ShellConfig::detect());
    let cancel = CancelToken::new();
    let mut child_pid: Option<u32> = None;
    let mut tracked_while_running = false;
    {
        // `$$` in the spawned shell IS the direct child's pid — the same value
        // `KillTreeOnDrop::arm` was handed — and `setsid` made it the group id too.
        let stopper = cancel.clone();
        let status = proc
            .exec(
                exec_spec("echo $$; sleep 30"),
                cancel.clone(),
                None,
                &mut |data: &[u8]| {
                    if child_pid.is_none()
                        && let Ok(pid) = String::from_utf8_lossy(data).trim().parse::<u32>()
                    {
                        child_pid = Some(pid);
                        tracked_while_running = is_tracked(pid);
                        stopper.cancel();
                    }
                },
            )
            .await
            .expect("exec runs");
        assert_eq!(
            status,
            ExitStatus::Killed,
            "fixture: the callback cancels, so this must be the cancel path"
        );
    }

    let pid = child_pid.expect("fixture: the shell must have reported its own pid");
    assert!(
        tracked_while_running,
        "a running detached bash child must be in the registry Pi's signal handlers drain \
         (pid {pid}) — otherwise `killTrackedDetachedChildren` has nothing to kill"
    );
    assert!(
        !is_tracked(pid),
        "the finished exec must have left the registry (Pi's `finally` untrack, bash.ts:142) — \
         a retained pid {pid} is worse than a forgotten one, since the next drain would \
         `killpg` a group this process no longer owns"
    );
}

/// The JS→Rust guarantee gap on the UNTRACK side, and why it lives in `Drop`.
///
/// Pi's untrack sits in a `finally` (`core/tools/bash.ts:142` @v0.83.0), so it runs on the
/// normal return, the `aborted` throw and the `timeout:` throw alike — an `async` function
/// always settles. A Rust future does not: dropping `exec` mid-`select!` (here via
/// `tokio::time::timeout`, equally a cancelled `tokio::spawn`, a panic, or runtime teardown)
/// skips everything written after the loop. An untrack placed on the success path would
/// therefore leak this pid for the life of the process, and the next
/// `kill_tracked_detached_children` would `killpg` a pid the kernel may have recycled onto an
/// unrelated group.
///
/// RED if the untrack moves next to `kill_guard.disarm()`; GREEN with it in `Drop`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_exec_future_untracks_its_pid_instead_of_leaking_it() {
    let proc = LocalProc::new(ShellConfig::detect());
    let mut child_pid: Option<u32> = None;
    let mut tracked_while_running = false;
    {
        let elapsed = tokio::time::timeout(
            Duration::from_millis(500),
            proc.exec(
                exec_spec("echo $$; sleep 30"),
                CancelToken::new(),
                None,
                &mut |data: &[u8]| {
                    if child_pid.is_none()
                        && let Ok(pid) = String::from_utf8_lossy(data).trim().parse::<u32>()
                    {
                        child_pid = Some(pid);
                        tracked_while_running = is_tracked(pid);
                    }
                },
            ),
        )
        .await;
        assert!(
            elapsed.is_err(),
            "fixture: the command must still be running when the timeout DROPS the future — \
             otherwise this observes a normal return, not the drop path"
        );
    }

    let pid = child_pid.expect("fixture: the shell must have reported its own pid");
    assert!(
        tracked_while_running,
        "fixture: the pid must have been in the registry before the drop, or the absence \
         assertion below is vacuous"
    );
    assert!(
        !is_tracked(pid),
        "an ABANDONED exec must still untrack pid {pid} — the untrack is Pi's `finally` \
         (bash.ts:142) and its only faithful Rust home is `Drop`, not a statement after the \
         `select!` loop that a dropped future never reaches"
    );
}

/// SEAM-S03, the drain half: `killTrackedDetachedChildren` (`utils/shell.ts:190-195` @v0.83.0)
/// must `killProcessTree` every registered pid — on unix `process.kill(-pid, "SIGKILL")`
/// (`:214`), the whole process GROUP, not just the leader — and empty the registry afterwards
/// (`:194`).
///
/// The discriminating assertion is the GRANDCHILD: a single-pid kill would leave the
/// backgrounded `sleep 30` running for its full 30s, which is exactly the orphan SEAM-S03 is
/// about. Its liveness is asserted BEFORE the drain so a fixture that never started cannot pass
/// this vacuously.
///
/// Runs against a registry this test owns rather than the process-global one — see
/// [`drain_and_kill`] for why.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_drain_sigkills_each_registered_group_and_empties_the_registry() {
    let marker = std::env::temp_dir().join(format!("cyrup-drain-{}-{}.pid", std::process::id(), 1));
    let _ = std::fs::remove_file(&marker);
    // Same fixture shape as the drop-guard test: a `setsid` leader that backgrounds a
    // descendant into its own group and then blocks.
    let spec = exec_spec(&format!("sleep 30 & echo $! > {}; wait", marker.display()));
    let mut cmd = tokio::process::Command::from(build_command(&spec));
    cmd.kill_on_drop(true);
    let mut leader_child = cmd.spawn().expect("fixture: the shell must spawn");
    let leader = leader_child
        .id()
        .expect("fixture: the shell must have a pid");
    // `build_command` only appends the command to argv under `Transport::Argv`; the WSL-legacy
    // `bash -s` config `try_detect` can return instead expects it on stdin (`shell.rs:52`), and
    // without this the shell would block on an open pipe and never start the fixture.
    if spec.shell.transport == Transport::Stdin
        && let Some(mut stdin) = leader_child.stdin.take()
    {
        let _ = stdin.write_all(spec.command.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let mut descendant = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Some(pid) = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            descendant = Some(pid);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = std::fs::remove_file(&marker);
    let descendant = descendant.expect("fixture: the descendant must have recorded its pid");
    assert!(
        pid_exists(descendant),
        "fixture: the backgrounded descendant (pid {descendant}) must be alive before the \
         drain, or its absence afterwards proves nothing"
    );

    let registry = std::sync::Mutex::new(std::collections::BTreeSet::from([leader]));
    drain_and_kill(&registry);

    assert!(
        registry.lock().map(|set| set.is_empty()).unwrap_or(false),
        "the drain must empty the registry (Pi's `trackedDetachedChildPids.clear()`, \
         shell.ts:194), so a second delivery does not re-signal recycled pids"
    );
    let group_died = wait_gone(descendant, Duration::from_secs(3)).await;
    // Clean up before asserting, so a failing run does not itself leak the `sleep 30`.
    let _ = kill_pid(descendant);
    let _ = leader_child.start_kill();
    assert!(
        group_died,
        "the drain must `killpg` the whole group: the backgrounded descendant (pid \
         {descendant}) outlived it, which is a single-pid kill of the leader ({leader}), not \
         Pi's `killProcessTree`"
    );
}
