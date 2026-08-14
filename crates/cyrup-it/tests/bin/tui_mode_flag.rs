//! SEAM-051 end-to-end — `--tui-mode` must not make the binary unlaunchable.
//!
//! `--tui-mode <regular|fullscreen>` is pi `cli/args.ts:180-192` @v0.84.1 (upstream drift: the flag
//! does not exist at v0.83.0, the tag cyrup ported — ADR-0005 decided cyrup ports it anyway). cyrup
//! had no entry for it in `KNOWN_LONG_FLAGS`, so `partition_extension_flags` captured it as an
//! extension flag and the reconciliation error `Unknown option: --tui-mode` exited 1 **before any
//! session was built** — i.e. the flag's own default value refused to launch the binary, so no pi
//! command line or wrapper script could start cyrup.
//!
//! Every assertion here is on the REAL binary, which is the only place the exit code and the stderr
//! text can be observed together. Fully offline, hermetic tempdir HOME/agent dir.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::process::{Command, Stdio};

use tempfile::TempDir;

struct Run {
    code: i32,
    stderr: String,
    stdout: String,
}

/// Run the real `cyrup` binary in a hermetic tempdir, offline, with no extensions. `args` is the
/// WHOLE user argv after the hermetic prefix — the `-p hi` tail is appended unless `bare` is set.
fn run_with(args: &[&str], bare: bool) -> (Run, TempDir) {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut cmd = Command::new(crate::support::bins::cyrup());
    cmd.current_dir(&work)
        .env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        // Never inherit an ambient built-in opt-in — see `unknown_flag_exit.rs` for why (a detached
        // `__intercom-broker` outlives the run).
        .env_remove("CYRUP_INTERCOM")
        .env_remove("CYRUP_SUBAGENTS")
        .env_remove("CYRUP_PERMISSION_SYSTEM")
        .args(["--offline", "--no-session", "--no-extensions"])
        .args(args);
    if !bare {
        cmd.args(["-p", "hi"]);
    }
    let out = cmd.stdin(Stdio::null()).output().expect("spawn cyrup");
    (
        Run {
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        },
        tmp,
    )
}

fn run(args: &[&str]) -> (Run, TempDir) {
    run_with(args, false)
}

/// THE headline: the flag's own default value must get PAST arg parsing. Before this fix every one
/// of these died with `Error: Unknown option: --tui-mode` and exit 1 before a session existed.
#[test]
fn tui_mode_regular_is_accepted_in_every_form_and_every_mode() {
    for args in [
        vec!["--tui-mode", "regular"],
        vec!["--tui-mode=regular"],
        vec!["--mode", "json", "--tui-mode", "regular"],
        vec!["--mode", "rpc", "--tui-mode", "regular"],
    ] {
        let (r, _tmp) = run(&args);
        assert!(
            !r.stderr.contains("Unknown option"),
            "{args:?} must not be an unknown option; stderr was: {}",
            r.stderr
        );
        assert!(
            !r.stderr.contains("Invalid TUI mode"),
            "{args:?} is pi's default value; stderr was: {}",
            r.stderr
        );
    }
}

/// pi args.ts:188-191 — the invalid-value text, verbatim, and an error exits 1 (main.ts:504-512).
#[test]
fn tui_mode_bogus_prints_pis_exact_invalid_value_text() {
    for args in [vec!["--tui-mode", "bogus"], vec!["--tui-mode=bogus"]] {
        let (r, _tmp) = run(&args);
        assert!(
            r.stderr
                .contains("Invalid TUI mode \"bogus\". Valid values: regular, fullscreen"),
            "{args:?} stderr was: {}",
            r.stderr
        );
        assert_eq!(r.code, 1, "{args:?} stderr was: {}", r.stderr);
        assert!(
            !r.stdout.contains("Invalid TUI mode"),
            "diagnostics go to stderr; stdout was: {}",
            r.stdout
        );
    }
}

/// pi args.ts:185-186 — a missing value, or a value that is itself a flag, is
/// `--tui-mode requires regular or fullscreen`. pi does NOT consume the next token on this branch,
/// so the following flag is still parsed.
#[test]
fn tui_mode_without_a_value_reports_pis_requires_text() {
    for args in [
        vec!["--tui-mode"],
        vec!["--tui-mode", "--verbose"],
        vec!["--tui-mode="],
    ] {
        let (r, _tmp) = run_with(&args, true);
        assert!(
            r.stderr
                .contains("Error: --tui-mode requires regular or fullscreen"),
            "{args:?} stderr was: {}",
            r.stderr
        );
        assert_eq!(r.code, 1, "{args:?} stderr was: {}", r.stderr);
    }
}

/// ADR-0005 §Decision A.2: `fullscreen` parses and is DECLINED at startup with the interim message
/// that names the ADR — not a pi diagnostic, not fatal (pi accepts the value, so exiting would
/// refuse a launch pi performs), and grep-able so work unit B-13 can delete it with the renderer.
#[test]
fn tui_mode_fullscreen_is_declined_with_the_adr_0005_interim_message() {
    let (r, _tmp) = run(&["--tui-mode", "fullscreen"]);
    assert!(
        r.stderr.contains(
            "--tui-mode fullscreen is not built yet in this release (ADR-0005); falling back to regular."
        ),
        "stderr was: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("Unknown option"),
        "stderr was: {}",
        r.stderr
    );
}

/// pi's help row, byte-for-byte at its upstream position (args.ts:291 @v0.84.1, between `--verbose`
/// and `--approve`). `--help` is also the one place the flag was invisible before.
#[test]
fn help_lists_tui_mode_at_pis_position() {
    let (r, _tmp) = run_with(&["--help"], true);
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
    assert!(
        r.stdout
            .contains("  --tui-mode <mode>              TUI mode: regular (default) or fullscreen\n"),
        "stdout was: {}",
        r.stdout
    );
    let verbose = r.stdout.find("  --verbose ").expect("--verbose row");
    let tui = r.stdout.find("  --tui-mode ").expect("--tui-mode row");
    let approve = r.stdout.find("  --approve, -a").expect("--approve row");
    assert!(verbose < tui && tui < approve, "stdout was: {}", r.stdout);
}
