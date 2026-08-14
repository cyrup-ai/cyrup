//! Resolves, ONCE for the whole integration suite:
//!
//!   * every workspace binary a seam test spawns (`cyrup`, the intercom broker, the three fixture
//!     binaries), and
//!   * the `wasm32-wasip2` guest component,
//!
//! then re-exports their absolute paths as `cargo::rustc-env=` so `env!()` resolves them in every
//! `[[test]]` target of this package.
//!
//! WHY THIS EXISTS AT ALL — the constraint that shapes the entire crate:
//!
//! `CARGO_BIN_EXE_<name>` is set only for test targets **in the same package as that binary**
//! ([cargo environment-variables]). It does not cross workspace members, so the 51 existing
//! `env!("CARGO_BIN_EXE_cyrup")` sites STOP COMPILING the day they land here. The repo already
//! documents the rule in its own source, at
//! `crates/cyrup-ext-subagents/tests/background_runner_main_integration.rs:103`. Cargo's own fix —
//! artifact dependencies, which set `CARGO_BIN_FILE_<DEP>_<NAME>` and explicitly work through
//! dev-dependencies — is nightly-only behind `-Z bindeps`, and `rust-toolchain.toml` pins stable.
//! A build script is the stable-Rust answer, and it is runner-agnostic: a nextest setup script
//! would make `$NEXTEST_ENV` load-bearing for CORRECTNESS, after which plain
//! `cargo test -p cyrup-it` stops working.
//!
//! It also collapses **24 nested `cargo build -p cyrup-ext-sdk --target wasm32-wasip2`
//! invocations** (13 in cyrup-ext, 10 in cyrup-session-svc, 1 in cyrup-tui) into one. Ten of those
//! share one fixed `$TMPDIR` path and five share another, so each group serializes on the other's
//! cargo build lock; four more (`cyrup-ext/tests/discover_load.rs:25`, `guest_host_mode.rs:36`,
//! `manifest_capabilities.rs:39`, `wasm_provider.rs:25`) pass no `--target-dir` at all and contend
//! for the WORKSPACE build lock — the exact contention their eight siblings were written to avoid.
//!
//! Cost, stated honestly: without `CYRUP_IT_BIN_DIR` this relinks five binaries into a private
//! target dir, a second compile of those graphs. It is paid once per suite invocation, not per
//! test, and only when the suite is armed. `CYRUP_IT_BIN_DIR` removes it entirely.
//!
//! [cargo environment-variables]: https://doc.rust-lang.org/cargo/reference/environment-variables.html

#![allow(clippy::expect_used, clippy::panic)] // a build script's only failure channel is a panic

use std::path::{Path, PathBuf};
use std::process::Command;

/// A binary the suite spawns: `(binary name, owning package, features that package needs)`.
///
/// The three fixture binaries are gated behind their own crate's `test-fixtures` feature
/// (`crates/cyrup-intercom/Cargo.toml:60`, `crates/cyrup-ext-subagents/Cargo.toml:102,112`) so
/// they are never compiled into, let alone shipped inside, the real `cyrup` binary. That gate is
/// why the nested build cannot be a bare `cargo build --bins`.
/// `cyrup`'s `faux` feature is NOT optional here, and the reason is subtle enough to be worth the
/// paragraph. `--model faux/faux-1` — which `tests/bin/{one_shot_parity, piped_stdin_trim,
/// unknown_flag_exit, extension_load_failure_exit, auth_credential_print}.rs` all drive a whole
/// offline turn through — reaches the scripted double only via `crates/cyrup/src/provider.rs`'s
/// `#[cfg(feature = "faux")]` arm, keyed to the **`cyrup` package's own** feature. In
/// `crates/cyrup/tests/`, where those five files used to live, it was on for free: cargo resolves
/// dev-dependencies for a test build, and `crates/cyrup/Cargo.toml`'s self-dev-dependency
/// `cyrup = { path = ".", features = ["faux"] }` unified it into that build. The invocation below
/// is a plain `cargo build`, which does NOT resolve dev-dependencies, so without this the spawned
/// binary answers `formatNoModelsAvailableMessage()` and five files red at runtime while
/// type-checking perfectly.
///
/// This does not weaken PROV-052. The gate that holds is the `#[cfg]` above, plus the fact that no
/// NORMAL edge enables the feature; `faux_not_in_normal_build.rs` asserts exactly that, over
/// `cargo tree -p cyrup -e features --edges normal`, and an explicit `--features` on a private
/// test-only build in `$OUT_DIR` is not an edge in that graph. `cargo build`, `cargo build
/// --release` and `cargo install` are all still faux-free.
///
/// Same trap on the `CYRUP_IT_BIN_DIR` shortcut: build that binary with
/// `cargo build -p cyrup --features faux --bin cyrup`, not a bare `--workspace --bins`.
const BINS: &[(&str, &str, &[&str])] = &[
    ("cyrup", "cyrup", &["faux"]),
    ("cyrup-intercom-broker", "cyrup-intercom", &["test-fixtures"]),
    ("cyrup-intercom-child-fixture", "cyrup-intercom", &["test-fixtures"]),
    ("cyrup-subagent-fixture", "cyrup-ext-subagents", &["test-fixtures"]),
    (
        "cyrup-subagent-orchestrator-sim",
        "cyrup-ext-subagents",
        &["test-fixtures"],
    ),
];

/// The guest crate built for `wasm32-wasip2`. The wasip2 linker componentizes directly, so no
/// `cargo-component` / `wasm-tools` step is needed (see `cyrup-ext/tests/wasm_component.rs:8-10`).
const WASM_PKG: &str = "cyrup-ext-sdk";
const WASM_TARGET: &str = "wasm32-wasip2";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=CYRUP_IT_BIN_DIR");
    println!("cargo::rerun-if-env-changed=CYRUP_EXT_FIXTURE_COMPONENT");

    // ---------------------------------------------------------------------------------------
    // No-op unless the suite is armed. Cargo sets `CARGO_FEATURE_<NAME>` for build scripts, so a
    // plain `cargo test --workspace` / `cargo check --workspace` pays NOTHING here: no nested
    // cargo, no wasm toolchain requirement, no second link. This is half of why the gate can be a
    // feature at all.
    // ---------------------------------------------------------------------------------------
    if std::env::var_os("CARGO_FEATURE_IT").is_none() {
        return;
    }

    // Armed. From here on, changes to the sources of the binaries we hand to the tests must force
    // a re-resolve, or the suite silently runs against a stale `cyrup`. Tracking the whole
    // `crates/` tree is conservative on purpose — the nested build below is incremental, so a
    // spurious rerun costs an up-to-date check, while a MISSED rerun costs a false green.
    let ws = workspace_root();
    println!("cargo::rerun-if-changed={}", ws.join("crates").display());
    println!("cargo::rerun-if-changed={}", ws.join("Cargo.lock").display());

    // ---------------------------------------------------------------------------------------
    // 1. Binaries.
    // ---------------------------------------------------------------------------------------
    if let Some(dir) = std::env::var_os("CYRUP_IT_BIN_DIR") {
        // The caller already built them (`cargo build --workspace --bins`, or CI's artifact
        // download) and is pointing us at the directory. Skip the second link entirely.
        let dir = PathBuf::from(dir);
        for (bin, _, _) in BINS {
            let path = dir.join(bin);
            if !path.exists() {
                // Deliberately a warning, not a panic: CYRUP_IT_BIN_DIR is an explicit override,
                // and the failure it produces downstream (`No such file or directory` from the
                // spawn) names the exact test that needed the binary. Failing the BUILD here would
                // block type-checking the suite on having built every binary first.
                println!(
                    "cargo::warning=CYRUP_IT_BIN_DIR is set but {} does not exist; tests that \
                     spawn it will fail at runtime",
                    path.display()
                );
            }
            emit_bin(bin, &path);
        }
    } else {
        // MUST use a target dir distinct from the outer build's. A nested cargo that shares the
        // workspace target dir contends for its build lock — and `crates/cyrup-ext/Cargo.toml:47-52`
        // records what that has already cost this repo (a leaked ~213 MB artifact cache filled a
        // 16 GB /tmp tmpfs and made `ld` die with SIGBUS while linking unrelated doctests).
        // `$OUT_DIR` is cleaned by `cargo clean`; the fixed `$TMPDIR` paths it replaces never were.
        let target_dir = out_dir().join("it-bins");
        let mut pending: Vec<&(&str, &str, &[&str])> = BINS.iter().collect();
        while let Some(head) = pending.first().copied() {
            // One cargo invocation per (package, feature-set): the three fixture bins come in two
            // groups, so this is three nested builds, not five.
            let (_, pkg, features) = *head;
            let group: Vec<&str> = pending
                .iter()
                .filter(|(_, p, f)| *p == pkg && *f == features)
                .map(|(b, _, _)| *b)
                .collect();
            pending.retain(|(_, p, f)| !(*p == pkg && *f == features));

            let built = cargo_build_bins(pkg, features, &group, &target_dir);
            for bin in &group {
                let path = built
                    .iter()
                    .find(|(name, _)| name == bin)
                    .map(|(_, path)| path.clone())
                    .unwrap_or_else(|| {
                        panic!(
                            "`cargo build -p {pkg}` produced no executable artifact named {bin}. \
                             Did its `required-features` change?"
                        )
                    });
                emit_bin(bin, &path);
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // 2. The WASM guest component. ONE build for the whole suite.
    // ---------------------------------------------------------------------------------------
    let component = match std::env::var_os("CYRUP_EXT_FIXTURE_COMPONENT") {
        // The escape hatch the 22 duplicated `fixture_component()` helpers each honour today, kept
        // working at ONE place instead of 22.
        Some(p) => PathBuf::from(p),
        None => cargo_build_component(WASM_PKG, WASM_TARGET, &out_dir().join("it-wasm")),
    };
    // Hard-fail with an actionable message. NEVER silently skip: `cyrup-tools/tests/build_tier1.rs`
    // currently returns GREEN when the toolchain is absent, which is a pass that proves nothing.
    assert!(
        component.exists(),
        "wasm guest component not found at {}. Run `rustup target add {WASM_TARGET}`, or set \
         CYRUP_EXT_FIXTURE_COMPONENT to a prebuilt component.",
        component.display()
    );
    println!(
        "cargo::rustc-env=CYRUP_IT_COMPONENT={}",
        component.display()
    );
}

/// Emit one binary path as compile-time env.
///
/// The name is UPPER_SNAKE_CASE (`cyrup-intercom-broker` -> `CYRUP_IT_BIN_CYRUP_INTERCOM_BROKER`),
/// a deliberate departure from §3.4's literal `CYRUP_IT_BIN_cyrup-intercom-broker`: a hyphenated,
/// case-sensitive env name is legal on Unix but is not a portable environment key. Tests should
/// not spell these out at all — go through `tests/support/bins.rs`, which owns the mapping.
fn emit_bin(bin: &str, path: &Path) {
    println!(
        "cargo::rustc-env=CYRUP_IT_BIN_{}={}",
        env_key(bin),
        path.display()
    );
}

fn env_key(bin: &str) -> String {
    bin.to_ascii_uppercase().replace(['-', '.'], "_")
}

/// `cargo build -p <pkg> --features <…> --bin <…>…`, returning `(binary name, absolute path)` for
/// every executable artifact the build reported.
///
/// Paths come from the JSON artifact stream — `"reason":"compiler-artifact"` records carry an
/// `executable` field — rather than from guessing at `target/debug/<name>`, which is wrong under a
/// custom profile, a cross target, or a `[build] target-dir` config.
fn cargo_build_bins(
    pkg: &str,
    features: &[&str],
    bins: &[&str],
    target_dir: &Path,
) -> Vec<(String, PathBuf)> {
    let mut cmd = base_cargo();
    cmd.args(["build", "-p", pkg]);
    if !features.is_empty() {
        cmd.args(["--features", &features.join(",")]);
    }
    for bin in bins {
        cmd.args(["--bin", bin]);
    }
    cmd.arg("--target-dir").arg(target_dir);

    let stdout = run_json(cmd, &format!("build {pkg} binaries"));
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let Some(exe) = v.get("executable").and_then(|e| e.as_str()) else {
            continue;
        };
        let Some(name) = v.pointer("/target/name").and_then(|n| n.as_str()) else {
            continue;
        };
        out.push((name.to_string(), PathBuf::from(exe)));
    }
    out
}

/// `cargo build -p <pkg> --target <triple>`, returning the single `.wasm` artifact.
fn cargo_build_component(pkg: &str, target: &str, target_dir: &Path) -> PathBuf {
    let mut cmd = base_cargo();
    cmd.args(["build", "-p", pkg, "--target", target]);
    cmd.arg("--target-dir").arg(target_dir);

    let stdout = run_json(cmd, &format!("build the {target} guest component"));
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        if v.pointer("/target/name").and_then(|n| n.as_str()) != Some(pkg) {
            continue;
        }
        if let Some(files) = v.get("filenames").and_then(|f| f.as_array()) {
            for f in files {
                let Some(f) = f.as_str() else { continue };
                if f.ends_with(".wasm") {
                    return PathBuf::from(f);
                }
            }
        }
    }
    panic!(
        "`cargo build -p {pkg} --target {target}` reported no .wasm artifact. Is the \
         `{target}` target installed (`rustup target add {target}`)?"
    );
}

/// A nested `cargo` with this build script's own cargo-injected environment removed.
///
/// `CARGO_ENCODED_RUSTFLAGS`, `RUSTC*` and the `CARGO_*` package variables describe THIS
/// compilation; letting them leak into the child makes it silently build with the wrong flags —
/// and, for `CARGO_ENCODED_RUSTFLAGS`, invalidates the child's cache on every invocation.
fn base_cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(workspace_root());
    cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
    cmd.env_remove("RUSTFLAGS");
    cmd.env_remove("RUSTC_WRAPPER");
    cmd.env_remove("RUSTC_WORKSPACE_WRAPPER");
    cmd.env_remove("CARGO_BUILD_TARGET_DIR");
    cmd.env_remove("CARGO_TARGET_DIR");
    cmd
}

/// Run a cargo command with the JSON message format, returning stdout.
///
/// `json-render-diagnostics` keeps human-readable errors on stderr (which cargo shows) while
/// stdout stays pure JSON for the artifact parse.
fn run_json(mut cmd: Command, what: &str) -> String {
    cmd.args(["--message-format", "json-render-diagnostics"]);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn cargo to {what}: {e}"));
    assert!(
        out.status.success(),
        "cargo failed to {what} ({}):\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is always set for a build script"))
}

/// `crates/cyrup-it` -> the workspace root. `CARGO_MANIFEST_DIR` is absolute, so this is stable
/// regardless of where the outer cargo was invoked from.
fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"),
    );
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or(manifest)
}
