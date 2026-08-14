//! EXT-S01, the fatal half — a contained extension load failure must be REPORTED and exit 1 in
//! **every** mode, not only in interactive.
//!
//! Pi contains the per-extension failure (`core/extensions/loader.ts:537-540`
//! `errors.push({path, error}); continue`) and then refuses to run: `main.ts:735-738` maps every
//! recorded error onto `runtime.diagnostics` as
//! `{type:"error", message:'Failed to load extension "<path>": <error>'}`, and `main.ts:843-849`
//! `reportDiagnostics(runtime.diagnostics)` + prints `EXTENSION_LOAD_FAILURE_HINT` (`main.ts:61`) +
//! `process.exit(1)`. That checkpoint sits before the mode dispatch, so it fires under print, json
//! and rpc exactly as it does under the TUI.
//!
//! cyrup's containment landed with the failure routed ONLY to `StartupDiagnostics::extensions`,
//! whose sole consumer is `build_startup_report` — called only from `run_interactive`. Under
//! `cyrup -p …` a built-in that failed `init()` therefore produced no message, no diagnostic and
//! exit 0, where the pre-containment code had at least died loudly. Those built-ins include the
//! permission gate, so the silent path is fail-OPEN.
//!
//! Only the real binary can show the exit code and the stderr text together, so that is what this
//! drives. Fully offline (`--offline`, faux model, tempdir HOME + agent dir, proxies stripped): no
//! network, no credentials.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

struct Run {
    code: i32,
    stderr: String,
    stdout: String,
}

/// Plant an extension the loader will discover and then fail to instantiate: a directory holding a
/// `*.wasm` artifact that is not a component. Discovery accepts it (`loader.rs::is_extension_dir`
/// synthesizes a manifest for a bare prebuilt `.wasm`); the load faults — Pi's `loadExtension`
/// catch arm (`loader.ts:487-491`).
fn plant_broken_extension(agent_dir: &Path) {
    let dir = agent_dir.join("extensions").join("broken-ext");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken-ext.wasm"), b"not a wasm component").unwrap();
}

/// Run the real `cyrup` binary in a hermetic tempdir with the offline faux model. `plant` decides
/// whether the broken extension exists — the only difference between the failing and control runs.
fn run(plant: bool, mode: &[&str]) -> (Run, TempDir) {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    if plant {
        plant_broken_extension(&agent_dir);
    }

    let mut cmd = Command::new(crate::support::bins::cyrup());
    cmd.current_dir(&work)
        .env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        // The extension set under test is the one this fixture PLANTS, so no ambient built-in
        // opt-in may join it. `CYRUP_INTERCOM=1` alone satisfies `is_installed()`
        // (`cyrup-intercom/src/extension.rs:630-631`, env var name at `:87`) despite the tempdir
        // agent dir holding no `intercom/config.json`, and the attached companion detaches an
        // immortal `__intercom-broker` — its shutdown check is armed only by a REGISTERED session's
        // disconnect (1:1 with pi-intercom `broker/broker.ts:221`/`:429`), which a one-shot child
        // never reaches. Measured: this crate's four binary-seam targets left 13 such processes per
        // run, 0 under `env -u CYRUP_INTERCOM`.
        .env_remove("CYRUP_INTERCOM")
        .env_remove("CYRUP_SUBAGENTS")
        .env_remove("CYRUP_PERMISSION_SYSTEM")
        .args(["--offline", "--no-session", "--model", "faux/faux-1"])
        .args(mode)
        .stdin(Stdio::null());
    let out = cmd.output().expect("spawn cyrup");
    (
        Run {
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        },
        tmp,
    )
}

/// THE headline: print mode — the mode with no startup panel at all — names the failed extension on
/// stderr and exits 1.
#[test]
fn a_failed_extension_load_reports_and_exits_1_in_print_mode() {
    let (r, _tmp) = run(true, &["-p", "hi"]);
    assert_eq!(r.code, 1, "stderr was: {}", r.stderr);
    assert!(
        r.stderr.contains("Error: Failed to load extension") && r.stderr.contains("broken-ext"),
        "stderr was: {}",
        r.stderr
    );
    // Pi prints its hint alongside (main.ts:844-846, EXTENSION_LOAD_FAILURE_HINT at :61).
    assert!(
        r.stderr.contains("Hint: Start without extensions using \"cyrup -ne\"."),
        "stderr was: {}",
        r.stderr
    );
    // The diagnostic belongs on stderr; stdout stays clean for the protocol/answer stream.
    assert!(!r.stdout.contains("Failed to load extension"), "stdout was: {}", r.stdout);
}

/// The same failure in `--mode json`: the machine-readable surface must not swallow it either.
#[test]
fn a_failed_extension_load_reports_and_exits_1_in_json_mode() {
    let (r, _tmp) = run(true, &["--mode", "json", "-p", "hi"]);
    assert_eq!(r.code, 1, "stderr was: {}", r.stderr);
    assert!(r.stderr.contains("Failed to load extension"), "stderr was: {}", r.stderr);
}

/// …and in rpc mode, which never renders a startup panel at all.
#[test]
fn a_failed_extension_load_reports_and_exits_1_in_rpc_mode() {
    let (r, _tmp) = run(true, &["--rpc"]);
    assert_eq!(r.code, 1, "stderr was: {}", r.stderr);
    assert!(r.stderr.contains("Failed to load extension"), "stderr was: {}", r.stderr);
}

/// The CONTROL that stops this from degrading into "every startup exits 1": the identical
/// invocation WITHOUT the broken extension gets past the checkpoint and into dispatch — proven by
/// the faux provider's own out-of-responses message, only reachable after the session is built and
/// prompted. Also proves `-ne` is genuinely the escape hatch the hint advertises.
#[test]
fn a_clean_run_passes_the_checkpoint_and_ne_is_a_real_escape_hatch() {
    let (clean, _tmp) = run(false, &["-p", "hi"]);
    assert!(
        !clean.stderr.contains("Failed to load extension"),
        "a clean agent dir must produce no load-failure diagnostic: {}",
        clean.stderr
    );
    assert!(
        clean.stderr.contains("No more faux responses queued"),
        "the run must reach dispatch, i.e. past the checkpoint; stderr: {}",
        clean.stderr
    );

    // The broken extension is present but `-ne` skips the discovery roots entirely.
    let (skipped, _tmp2) = run(true, &["-ne", "-p", "hi"]);
    assert!(
        !skipped.stderr.contains("Failed to load extension"),
        "`-ne` must skip the broken extension; stderr: {}",
        skipped.stderr
    );
    assert!(
        skipped.stderr.contains("No more faux responses queued"),
        "`-ne` must reach dispatch; stderr: {}",
        skipped.stderr
    );
}
