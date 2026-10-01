//! The placed-run relay against a real process boundary: an ssh stand-in that runs the remote
//! command in a local `/bin/sh` (the machine), a run directory a scripted "child" writes into the
//! way a placed `run.sh` does, and the relay reading it back through [`crate::relay::RunRelay`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::relay::{DownMirror, MirrorKind, RelayEnd, RunRelay, UpMirror};
use crate::remote::SshTransport;

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// An ssh that "reaches" the machine by running the remote command in a local shell. When
/// `drop_first_stream_after` is set, the FIRST relay stream it carries is cut after that many
/// bytes and the ssh exits 255 — a dropped connection mid-frame.
fn fake_ssh(dir: &Path, drop_first_stream_after: Option<usize>) -> SshTransport {
    let counter = dir.join("streams");
    let cut = drop_first_stream_after
        .map(|bytes| {
            format!(
                "case \"$1\" in *relay-snap*) n=$(cat '{c}' 2>/dev/null || echo 0); n=$((n+1)); echo $n > '{c}'; if [ \"$n\" = 1 ]; then /bin/sh -c \"$1\" | head -c {bytes}; exit 255; fi;; esac",
                c = counter.display()
            )
        })
        .unwrap_or_default();
    let ssh = script(
        dir,
        "fake-ssh",
        &format!(
            "while [ $# -gt 0 ]; do case \"$1\" in -o) shift 2;; -T|-N) shift;; *) break;; esac; done\n[ \"$1\" = me@box ] || {{ echo \"ssh: Could not resolve hostname $1\" >&2; exit 255; }}\nshift\n{cut}\nexec /bin/sh -c \"$1\""
        ),
    );
    SshTransport::new(ssh.display().to_string(), None)
}

/// A run directory named the way a placed run's is, with its identity file.
fn run_dir(root: &Path, id: &str) -> PathBuf {
    let rt = root.join(format!("cyrup-subagents-herdr-{id}-abcdefgh"));
    std::fs::create_dir_all(rt.join("supervisor/requests")).unwrap();
    std::fs::create_dir_all(rt.join("supervisor/replies")).unwrap();
    std::fs::create_dir_all(rt.join("steer-acks")).unwrap();
    std::fs::write(rt.join("run-id"), id).unwrap();
    rt
}

/// Start a scripted placed child in `rt`: its pid, `started`, then `body`.
fn start_child(rt: &Path, body: &str) -> std::process::Child {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "rt='{}'; echo $$ > \"$rt/pid\"; : > \"$rt/started\"\n{body}",
            rt.display()
        ))
        .spawn()
        .unwrap()
}

async fn run(relay: RunRelay) -> (RelayEnd, Vec<u8>, String) {
    let (mut events_read, events_write) = tokio::io::duplex(1 << 20);
    let (mut stderr_read, stderr_write) = tokio::io::duplex(1 << 16);
    let task = tokio::spawn(relay.run(events_write, stderr_write));
    let read_events = tokio::spawn(async move {
        let mut out = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut events_read, &mut out)
            .await
            .unwrap();
        out
    });
    let mut stderr = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stderr_read, &mut stderr)
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .unwrap()
        .unwrap();
    (
        end,
        read_events.await.unwrap(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

/// THE relay contract: every event byte the child wrote arrives once, in order; a supervisor
/// request the child wrote appears in the parent's channel; the parent's reply is DELIVERED into
/// the run directory (uploaded, then removed locally) where the waiting child reads it; the child's
/// acknowledgement comes back; its stderr tail and exit code end the relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_streams_events_and_mirrors_both_channel_directions() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run_dir(dir.path(), "run-one");
    let local = dir.path().join("local");
    let requests = local.join("requests");
    let replies = local.join("replies");
    let acks = local.join("acks");
    std::fs::create_dir_all(&requests).unwrap();
    std::fs::create_dir_all(&replies).unwrap();
    let mut child = start_child(
        &rt,
        r#"printf '{"n":1}\n' >> "$rt/events.ndjson"
printf '{"id":"q1"}' > "$rt/supervisor/requests/q1.json"
i=0; while [ ! -f "$rt/supervisor/replies/q1.json" ]; do i=$((i+1)); [ $i -gt 300 ] && exit 9; sleep 0.1; done
printf '{"n":2,"reply":%s}\n' "$(cat "$rt/supervisor/replies/q1.json")" >> "$rt/events.ndjson"
printf '{"ack":"s1"}' > "$rt/steer-acks/s1.json"
echo "child diagnostics" > "$rt/stderr"
echo 3 > "$rt/exit""#,
    );
    let relay = RunRelay::new(
        fake_ssh(dir.path(), None),
        "me@box",
        rt.display().to_string(),
        "run-one",
    )
    .with_down(DownMirror {
        key: "supervisor-request".to_string(),
        remote: "supervisor/requests".to_string(),
        local: requests.clone(),
        kind: MirrorKind::Dir,
    })
    .with_down(DownMirror {
        key: "steer-ack".to_string(),
        remote: "steer-acks".to_string(),
        local: acks.clone(),
        kind: MirrorKind::Dir,
    })
    .with_up(UpMirror {
        local: replies.clone(),
        remote: "supervisor/replies".to_string(),
    });
    // The parent answers once the request reaches its channel.
    let answer = {
        let requests = requests.clone();
        let replies = replies.clone();
        tokio::spawn(async move {
            for _ in 0..600 {
                if requests.join("q1.json").is_file() {
                    std::fs::write(replies.join("q1.json"), "\"yes\"").unwrap();
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            false
        })
    };
    let (end, events, stderr) = run(relay).await;
    assert!(
        answer.await.unwrap(),
        "the request never reached the parent"
    );
    let _ = child.wait();
    assert_eq!(end, RelayEnd::Exited(3));
    assert_eq!(
        String::from_utf8(events).unwrap(),
        "{\"n\":1}\n{\"n\":2,\"reply\":\"yes\"}\n"
    );
    assert!(stderr.contains("child diagnostics"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(requests.join("q1.json")).unwrap(),
        "{\"id\":\"q1\"}"
    );
    assert_eq!(
        std::fs::read_to_string(acks.join("s1.json")).unwrap(),
        "{\"ack\":\"s1\"}"
    );
    assert!(
        !replies.join("q1.json").exists(),
        "a delivered reply leaves the parent's outbox"
    );
}

/// A connection cut mid-stream (ssh exits 255 after part of a frame) is reconnected and resumes
/// from the byte offset already delivered: no event is lost or repeated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_stream_resumes_from_the_delivered_offset() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run_dir(dir.path(), "run-two");
    let lines: String = (0..40).map(|n| format!("{{\"line\":{n}}}\n")).collect();
    std::fs::write(rt.join("events.ndjson"), &lines).unwrap();
    let mut child = start_child(&rt, "sleep 1; echo 0 > \"$rt/exit\"");
    let relay = RunRelay::new(
        fake_ssh(dir.path(), Some(70)),
        "me@box",
        rt.display().to_string(),
        "run-two",
    );
    let (end, events, _) = run(relay).await;
    let _ = child.wait();
    assert_eq!(end, RelayEnd::Exited(0));
    assert_eq!(String::from_utf8(events).unwrap(), lines);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("streams"))
            .unwrap()
            .trim(),
        "2",
        "exactly one reconnect"
    );
}

/// A child whose `run.sh` died without recording an exit — its pane was closed or killed — is
/// reported lost rather than waited on forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_whose_pane_died_is_reported_lost() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run_dir(dir.path(), "run-three");
    let mut child = start_child(
        &rt,
        "printf '{\"n\":1}\\n' >> \"$rt/events.ndjson\"; exit 0",
    );
    let _ = child.wait();
    let relay = RunRelay::new(
        fake_ssh(dir.path(), None),
        "me@box",
        rt.display().to_string(),
        "run-three",
    );
    let (end, events, stderr) = run(relay).await;
    assert!(
        matches!(&end, RelayEnd::Lost(reason) if reason.contains("its Herdr pane was closed or killed")),
        "{end:?}"
    );
    assert_eq!(String::from_utf8(events).unwrap(), "{\"n\":1}\n");
    assert!(stderr.contains("closed or killed"), "{stderr}");
}

/// A transport that never comes back is unknown after upstream's budget; a directory that is not
/// this run's is refused at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_transport_is_unknown_and_a_foreign_run_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run_dir(dir.path(), "run-four");
    let unreachable = RunRelay::new(
        fake_ssh(dir.path(), None),
        "me@elsewhere",
        rt.display().to_string(),
        "run-four",
    );
    let started = std::time::Instant::now();
    let (end, _, _) = run(unreachable).await;
    assert!(
        matches!(&end, RelayEnd::Unknown(reason)
            if reason.starts_with("Pane-native reconnect remains unknown:")
                && reason.contains("Could not resolve hostname")),
        "{end:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(15));

    let foreign = RunRelay::new(
        fake_ssh(dir.path(), None),
        "me@box",
        rt.display().to_string(),
        "someone-else",
    );
    let (end, _, _) = run(foreign).await;
    assert_eq!(
        end,
        RelayEnd::Unknown(
            "Pane-native reconnect remains unknown: The placed run's identity changed.".to_string()
        )
    );
}

/// The remote relay shells under test: dash (`/bin/sh` here), and bash when it is installed —
/// the script is written for any POSIX `sh`, and a machine's `sh` is whichever it ships.
fn relay_shells() -> Vec<&'static str> {
    ["/bin/sh", "/usr/bin/bash", "/bin/bash"]
        .into_iter()
        .filter(|shell| Path::new(shell).exists())
        .collect()
}

/// HERDR-006: killing the local ssh client ends no remote command, so the relay's remote half
/// outlives a run that was cleaned up — unless it notices that its runtime directory is gone.
/// Here the "remote half" is started directly (as `sh -c`, exactly the command the ssh carries),
/// against a child that never exits; the directory is then moved away the way the placed run's
/// `REMOVE_SCRIPT` does, and the relay must end within two seconds (it polls every 200 ms), with
/// the refusal status the script uses for a missing directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_remote_relay_ends_when_its_runtime_dir_is_removed() {
    for shell in relay_shells() {
        let dir = tempfile::tempdir().unwrap();
        let rt = run_dir(dir.path(), "run-gone");
        let mut child = start_child(&rt, "exec sleep 60");
        let command = crate::remote::remote_shell_command(
            crate::relay::RELAY_SCRIPT,
            &[&rt.display().to_string(), "run-gone", "0"],
        );
        // The command is `sh -c '…' sh …`; run it under the shell being tested, `exec`ed so that
        // the test's handle IS the relay (killing it on failure leaves nothing behind). stdout is
        // a pipe held open by the test, so a broken pipe cannot end the loop by accident — the
        // only thing left to end it is the directory check.
        let mut remote = std::process::Command::new(shell)
            .arg("-c")
            .arg(format!(
                "exec {}",
                command.replacen("sh -c", &format!("{shell} -c"), 1)
            ))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // In its main loop once it has created its snapshot state file (`.relay-old.<its pid>`;
        // the pid is the inner `sh`'s, not the `Child`'s, so look for the prefix).
        let in_loop = |rt: &Path| {
            std::fs::read_dir(rt)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".relay-old.")
                })
        };
        for _ in 0..100 {
            if in_loop(&rt) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(in_loop(&rt), "{shell}: the relay never reached its loop");

        std::fs::rename(&rt, dir.path().join("removed")).unwrap();

        let mut status = None;
        for _ in 0..40 {
            if let Some(done) = remote.try_wait().unwrap() {
                status = Some(done);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = remote.kill();
        let _ = remote.wait();
        let _ = child.kill();
        let _ = child.wait();
        let status = status.unwrap_or_else(|| {
            panic!("{shell}: the relay was still running 2 s after its runtime dir was removed")
        });
        assert_eq!(status.code(), Some(66), "{shell}: {status:?}");
    }
}

/// The same check covers the wait for `started`: a run whose directory is removed before its child
/// ever starts must not leave the relay counting down its 60 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_remote_relay_does_not_wait_out_the_start_deadline_for_a_removed_dir() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run_dir(dir.path(), "run-unstarted");
    let command = crate::remote::remote_shell_command(
        crate::relay::RELAY_SCRIPT,
        &[&rt.display().to_string(), "run-unstarted", "0"],
    );
    let mut remote = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("exec {command}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        remote.try_wait().unwrap().is_none(),
        "the relay waits for `started`"
    );
    std::fs::rename(&rt, dir.path().join("removed")).unwrap();
    let mut status = None;
    for _ in 0..40 {
        if let Some(done) = remote.try_wait().unwrap() {
            status = Some(done);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = remote.kill();
    let _ = remote.wait();
    assert_eq!(
        status.expect("ended within 2 s").code(),
        Some(66),
        "the start wait ends on a removed dir"
    );
}

/// HERDR-006: an upload never builds a runtime directory. Delivering into a channel of a removed
/// run fails (and stays queued locally for the next poll) and leaves nothing behind on the
/// machine; the same upload into a run that has its channel dir is delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upload_into_a_removed_run_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run_dir(dir.path(), "run-upload");
    let outbox = dir.path().join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    std::fs::write(outbox.join("r1.json"), "\"yes\"").unwrap();
    let mirror = UpMirror {
        local: outbox.clone(),
        remote: "supervisor/replies".to_string(),
    };
    let relay = RunRelay::new(
        fake_ssh(dir.path(), None),
        "me@box",
        rt.display().to_string(),
        "run-upload",
    );

    // The run is cleaned up, then the parent's reply is uploaded.
    std::fs::rename(&rt, dir.path().join("removed")).unwrap();
    relay.upload_dir(&mirror).await;
    assert!(
        !rt.exists(),
        "the upload recreated the removed runtime dir: {:?}",
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    );
    assert!(
        outbox.join("r1.json").exists(),
        "a failed upload stays queued for the next poll"
    );

    // Control: a live run with its channel dir receives it, atomically, and the outbox drains.
    std::fs::rename(dir.path().join("removed"), &rt).unwrap();
    relay.upload_dir(&mirror).await;
    assert_eq!(
        std::fs::read_to_string(rt.join("supervisor/replies/r1.json")).unwrap(),
        "\"yes\""
    );
    assert!(!outbox.join("r1.json").exists());
}
