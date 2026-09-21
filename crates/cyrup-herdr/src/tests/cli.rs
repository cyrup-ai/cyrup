//! [`crate::cli::HerdrCli`] — the CLI fallback, against a fake `herdr` on disk.
//!
//! The `herdr` binary is not installed in this container and must not be needed: every test below
//! either drives the **absent-binary** path or writes a short shell script that prints the exact
//! bytes herdr's own CLI prints (`tmp/herdr/src/cli.rs:745-752` — the whole response envelope
//! on stdout, or the error envelope on stderr with exit 1) and runs that. A script is the honest
//! stand-in here for the same reason `FakeHerdr` is on the socket side: what is under test is this
//! client's reading of the bytes, and the bytes are herdr's, taken from its source.

use std::collections::BTreeMap;

use crate::cli::{HerdrCli, json_to_string};
use crate::error::ApiErrorCode;

#[test]
fn the_binary_ladder_reads_herdr_bin_and_falls_through_a_blank_one() {
    let unset: BTreeMap<String, String> = BTreeMap::new();
    assert_eq!(HerdrCli::with_env(&unset).bin(), "herdr");

    let set = BTreeMap::from([("HERDR_BIN".to_string(), "  /opt/herdr  ".to_string())]);
    assert_eq!(HerdrCli::with_env(&set).bin(), "/opt/herdr");

    // `HERDR_BIN=` in a shell profile is a variable that was unset badly, not a request to exec
    // the empty string.
    let blank = BTreeMap::from([("HERDR_BIN".to_string(), "   ".to_string())]);
    assert_eq!(HerdrCli::with_env(&blank).bin(), "herdr");
}

#[test]
fn json_to_string_renders_a_non_string_as_its_json() {
    // A JSON string yields itself, WITHOUT the quotes — the case `--version` takes.
    assert_eq!(
        json_to_string(&serde_json::json!("herdr 0.9.1")),
        "herdr 0.9.1"
    );
    // `[CYRUP-DELTA]`: JS `String({})` is `"[object Object]"`. The JSON form is what a reader can
    // act on.
    assert_eq!(
        json_to_string(&serde_json::json!({"version": "0.9.1"})),
        r#"{"version":"0.9.1"}"#
    );
    assert_eq!(json_to_string(&serde_json::json!(22)), "22");
}

/// Every named code round-trips its wire spelling — a property `cyrup-intercom` now depends on.
///
/// `HerdrLauncher::rendered` (`crates/cyrup-intercom/src/project_pane.rs`) hands pi's
/// `normalizeCode` the code it gets back from THIS enum, via [`ApiErrorCode::as_str`], rather than
/// the raw JSON string it used to read for itself. `normalizeCode` matches substrings —
/// `timeout`, `gone`, `not_found`, `not-found`, `no_such_pane` — so a spelling that did not survive
/// `from_wire` → `as_str` would silently change which of upstream's five codes a user is shown,
/// with nothing else to notice it. [`ApiErrorCode::Other`] round-trips by construction and is
/// pinned separately (`probe_round_trip.rs:159`); these are the 31 that do not.
#[test]
fn every_named_error_code_round_trips_its_wire_spelling() {
    for wire in [
        "invalid_request",
        "pane_not_found",
        "no_active_pane",
        "no_active_workspace",
        "stale_pane_target",
        "pane_split_failed",
        "pane_send_failed",
        "tab_not_found",
        "workspace_not_found",
        "invalid_params",
        "invalid_metadata_source",
        "invalid_metadata_token",
        "invalid_metadata_ttl",
        "invalid_agent",
        "invalid_key",
        "invalid_env",
        "confirmation_required",
        "invalid_regex",
        "agent_not_found",
        "agent_target_ambiguous",
        "invalid_agent_view",
        "agent_blocked",
        "feature_disabled",
        "stream_conflict",
        "query_too_large",
        "ui_busy",
        "stale_content",
        "timeout",
        "server_unavailable",
        "connection_local_only",
        "internal_error",
    ] {
        let decoded = ApiErrorCode::from_wire(wire);
        assert!(
            !matches!(decoded, ApiErrorCode::Other(_)),
            "{wire} is a named code, not Other"
        );
        assert_eq!(decoded.as_str(), wire, "{wire} did not round-trip");
    }
}

#[cfg(unix)]
mod spawned {
    use crate::cli::{CliError, HerdrCli, extract_pane_id};
    use crate::error::{ApiErrorCode, Unavailable};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// Long enough that a passing test never races it, short enough that a broken one still ends.
    ///
    /// Declared **inside** this `#[cfg(unix)]` module rather than at file scope because every user
    /// of it is here. At file scope a Windows build of the test target — the platform this crate
    /// was made a leaf to keep buildable, see `Cargo.toml` — compiles `mod cli` for its three
    /// platform-independent tests, finds nothing reading this const, and fails
    /// `cargo clippy --workspace --all-targets -- -D warnings` with `constant \`GENEROUS\` is
    /// never used`.
    const GENEROUS: Duration = Duration::from_secs(20);

    /// A `herdr` that prints exactly what `body` says and exits how it says.
    fn fake_herdr(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-herdr");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        wait_until_executable(&path);
        path
    }

    /// Block until `path` can actually be `exec`'d, i.e. until no process holds it open for
    /// writing.
    ///
    /// `cargo test` runs a crate's tests as **threads in one process**. When this thread calls
    /// `std::fs::write` on the fake binary, any sibling test that `fork`s during that window
    /// inherits a copy of the write fd, and the kernel answers `ETXTBSY` (`Text file busy`,
    /// `os error 26`) to an `exec` of a file any process still has open for writing. The failure
    /// surfaces as a spawn error from a completely unrelated test, so it reads as if the code
    /// under test were broken.
    ///
    /// One successful `exec` proves no such fd exists any more, and none can appear again —
    /// nothing writes this path after this point, and `fork` only copies fds that already exist.
    /// So a single successful spawn here makes every later spawn of this file safe for the rest
    /// of the process. The child is killed immediately: what is being probed is the `exec`, not
    /// whatever the script goes on to do.
    fn wait_until_executable(path: &Path) {
        for _ in 0..500 {
            match std::process::Command::new(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                // `ETXTBSY` is 26 on every unix this runs on; `libc` is not a dependency of this
                // crate and one constant does not justify making it one.
                Err(err) if err.raw_os_error() == Some(26) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                // Anything else is the spawning test's own business to report.
                Err(_) => return,
            }
        }
    }

    #[tokio::test]
    async fn an_absent_binary_is_binary_missing_and_names_what_was_tried() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("herdr-is-not-installed-here");
        let cli = HerdrCli::new(missing.display().to_string());

        let error = cli
            .run(&["pane", "list"], GENEROUS)
            .await
            .expect_err("a binary that does not exist cannot answer");

        match error {
            CliError::Unavailable(Unavailable::BinaryMissing { bin }) => {
                // The name is in the error because "herdr is not installed" is not actionable when
                // HERDR_BIN points somewhere stale, and the path is.
                assert_eq!(bin, missing.display().to_string());
            }
            other => panic!("expected BinaryMissing, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_successful_verb_yields_herdrs_result_frame_unwrapped() {
        let dir = tempfile::tempdir().unwrap();
        // `print_response` (`tmp/herdr/src/cli.rs:745-752`) writes the WHOLE response value, and a
        // `pane.split` answer is `{"type":"pane_info","pane":{"pane_id":…}}`
        // (`tmp/herdr/src/api/schema/panes.rs:527-528`). The noise line is what a herdr that logs
        // before answering would add.
        let bin = fake_herdr(
            dir.path(),
            r#"echo 'opening pane...'
echo '{"id":"cli:request","result":{"type":"pane_info","pane":{"pane_id":"w1:p2","focused":true}}}'"#,
        );

        let output = HerdrCli::new(bin.display().to_string())
            .run(&["pane", "split", "--current"], GENEROUS)
            .await
            .expect("the fake herdr answered");

        let json = output.json.expect("stdout carried an envelope");
        // Unwrapped: the caller sees the same object the socket would have handed it, so
        // `extract_pane_id` finds the pane and not the request id.
        assert_eq!(json.get("type").and_then(|t| t.as_str()), Some("pane_info"));
        assert_eq!(extract_pane_id(&json).as_deref(), Some("w1:p2"));
        // `text` is upstream's `textOk` arm and keeps the noise line, because a verb whose answer
        // is not JSON at all (`--version`) has nothing else.
        assert!(
            output.text.starts_with("opening pane..."),
            "{}",
            output.text
        );
    }

    #[tokio::test]
    async fn an_error_envelope_outranks_a_zero_exit() {
        let dir = tempfile::tempdir().unwrap();
        // Exit 0 AND an error envelope: herdr's own CLI does not do this, but `send_ok_request`
        // (`tmp/herdr/src/cli.rs:755-766`) shows how close the two paths are, and upstream's rule
        // (`project-agent.ts:118-123`) is that the envelope wins in both directions.
        let bin = fake_herdr(
            dir.path(),
            r#"echo '{"id":"cli:request","error":{"code":"pane_not_found","message":"pane w1:p9 not found"}}'
exit 0"#,
        );

        let error = HerdrCli::new(bin.display().to_string())
            .run(&["pane", "get", "w1:p9"], GENEROUS)
            .await
            .expect_err("an error envelope is a failure whatever the exit code");

        match error {
            CliError::Api {
                command,
                code,
                message,
            } => {
                assert_eq!(command, "pane get w1:p9");
                assert_eq!(code, ApiErrorCode::PaneNotFound);
                // herdr's own message, byte for byte — never a synthesised sentence.
                assert_eq!(message.as_deref(), Some("pane w1:p9 not found"));
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_cli_only_code_survives_as_other_from_stderr() {
        let dir = tempfile::tempdir().unwrap();
        // `protocol_mismatch` is produced by the CLI's protocol guard
        // (`tmp/herdr/src/cli/protocol_guard.rs:16-41`) and by nothing on the socket, so it is
        // exactly the code this crate must carry through rather than name — and this is the only
        // path on which a cyrup process can ever meet it.
        let bin = fake_herdr(
            dir.path(),
            r#"echo '{"id":"cli:request","error":{"code":"protocol_mismatch","message":"client protocol 21 is older than server protocol 22; upgrade the Herdr client before using this command"}}' >&2
exit 1"#,
        );

        let error = HerdrCli::new(bin.display().to_string())
            .run(&["pane", "list"], GENEROUS)
            .await
            .expect_err("a protocol mismatch is a failure");

        match error {
            CliError::Api { code, message, .. } => {
                assert_eq!(code, ApiErrorCode::Other("protocol_mismatch".to_string()));
                assert_eq!(
                    message.as_deref(),
                    Some(
                        "client protocol 21 is older than server protocol 22; upgrade the Herdr \
                         client before using this command"
                    )
                );
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_json_line_on_stderr_is_ignored_when_the_verb_succeeded() {
        let dir = tempfile::tempdir().unwrap();
        // A herdr that logs JSON to stderr and still succeeds must not be read as having failed.
        // stdout deliberately carries nothing parsable, so the ONLY envelope in reach is the one on
        // stderr — and it is out of reach, because stderr is consulted only on a non-zero exit
        // (`project-agent.ts:118`).
        let bin = fake_herdr(
            dir.path(),
            r#"echo '{"error":{"code":"internal_error","message":"a log line, not an answer"}}' >&2
echo 'pane closed'"#,
        );

        let output = HerdrCli::new(bin.display().to_string())
            .run(&["pane", "close", "w1:p2"], GENEROUS)
            .await
            .expect("a zero exit is a success whatever herdr logged");
        assert!(output.json.is_none(), "{:?}", output.json);
        assert_eq!(output.text, "pane closed");
    }

    /// The fixture prints **two** non-blank stderr lines, and only the first may be reported.
    ///
    /// With one content line, `find` and `rev().find()` return the same string and the test's own
    /// name is unpinned — a herdr that printed a usage summary under its error would surface the
    /// wrong sentence as `PaneErrorCode::ValidationError`'s message, diverging from upstream's
    /// `stderr.split(/\r?\n/).find((line) => line.trim())` (`project-agent.ts:133`) with nothing
    /// in the tree to notice. The leading blank line pins the blank-skipping and the surrounding
    /// spaces pin the trim.
    #[tokio::test]
    async fn a_bare_non_zero_exit_carries_the_first_non_blank_stderr_line_or_the_status() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_herdr(
            dir.path(),
            r#"echo '' >&2
echo '   error: unexpected argument   ' >&2
echo '' >&2
echo 'usage: herdr pane <split|read|close>' >&2
exit 2"#,
        );
        let error = HerdrCli::new(bin.display().to_string())
            .run(&["pane", "nope"], GENEROUS)
            .await
            .expect_err("exit 2 is a failure");
        match error {
            CliError::Exit {
                command,
                status,
                stderr_line,
            } => {
                assert_eq!(command, "pane nope");
                assert_eq!(status, Some(2));
                // The FIRST NON-BLANK line, trimmed — a leading empty line is not the reason,
                // and neither is the usage summary printed after it.
                assert_eq!(stderr_line.as_deref(), Some("error: unexpected argument"));
            }
            other => panic!("expected Exit, got {other:?}"),
        }

        let silent_dir = tempfile::tempdir().unwrap();
        let silent = fake_herdr(silent_dir.path(), "exit 7");
        let error = HerdrCli::new(silent.display().to_string())
            .run(&["pane", "list"], GENEROUS)
            .await
            .expect_err("exit 7 is a failure");
        match error {
            CliError::Exit {
                status,
                stderr_line,
                ..
            } => {
                assert_eq!(status, Some(7));
                // Nothing is invented: a consumer that must print something supplies its own text.
                assert_eq!(stderr_line, None);
            }
            other => panic!("expected Exit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_verb_that_never_finishes_times_out_and_its_child_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("finished");
        let bin = fake_herdr(
            dir.path(),
            &format!("sleep 4\necho done > {}", marker.display()),
        );

        let started = Instant::now();
        let error = HerdrCli::new(bin.display().to_string())
            .run(&["pane", "get"], Duration::from_millis(200))
            .await
            .expect_err("a herdr that never answers must not hang the caller");
        let elapsed = started.elapsed();

        match error {
            CliError::TimedOut { command, timeout } => {
                assert_eq!(command, "pane get");
                assert_eq!(timeout, Duration::from_millis(200));
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
        // The deadline is this client's, not the child's: it fires while the child still runs.
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");

        // `kill_on_drop` is the `child.kill()` upstream's timeout handler calls. Give the script
        // longer than its own sleep to prove it never got to run its second line.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !marker.exists(),
            "the timed-out child kept running and completed its work"
        );
    }

    #[tokio::test]
    async fn a_cancelled_run_is_cancelled_rather_than_timed_out() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_herdr(dir.path(), "sleep 4");

        let started = Instant::now();
        let error = HerdrCli::new(bin.display().to_string())
            // The deadline is 10 s away, so a prompt return proves the cancellation won the race
            // rather than the timeout.
            .run_cancellable(&["pane", "split"], Duration::from_secs(10), async {})
            .await
            .expect_err("a withdrawn caller gets no output");

        match error {
            // A withdrawn caller is not a slow herdr, and folding the two would report a herdr
            // fault for a cyrup decision.
            CliError::Cancelled { command } => assert_eq!(command, "pane split"),
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(2), "not prompt");
    }

    #[tokio::test]
    async fn version_text_reads_the_plain_line_and_a_json_payload_alike() {
        let plain_dir = tempfile::tempdir().unwrap();
        // What clap prints today (`tmp/herdr/Cargo.toml:3` is `version = "0.9.1"`).
        let plain = fake_herdr(plain_dir.path(), "echo 'herdr 0.9.1'");
        let text = HerdrCli::new(plain.display().to_string())
            .version_text(GENEROUS, std::future::pending())
            .await
            .expect("the fake herdr has a version");
        assert_eq!(text, "herdr 0.9.1");
        assert_eq!(crate::cli::parse_herdr_version(&text), Some((0, 9, 1)));

        let json_dir = tempfile::tempdir().unwrap();
        // A herdr that learns to answer `--version` as an envelope must not start reporting
        // `{"id":…}` as its version string.
        let json = fake_herdr(
            json_dir.path(),
            r#"echo '{"id":"cli:request","result":"herdr 1.2.3"}'"#,
        );
        let text = HerdrCli::new(json.display().to_string())
            .version_text(GENEROUS, std::future::pending())
            .await
            .expect("the fake herdr has a version");
        assert_eq!(text, "herdr 1.2.3");
        assert_eq!(crate::cli::parse_herdr_version(&text), Some((1, 2, 3)));
    }
}
