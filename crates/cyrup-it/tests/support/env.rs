//! Child-process environment hygiene.
//!
//! Two hard rules, both from docs/TEST-ARCHITECTURE.md §4:
//!
//! * **R2 — nothing here calls `std::env::set_var`/`remove_var`.** They became `unsafe` in edition
//!   2024 and are unsound in a multithreaded process, which every consolidated test binary is. The
//!   33 per-file `static ENV_MUTATION_LOCK`s in the old corpus were 33 *different* mutexes
//!   guarding one shared environment the moment two files shared a binary — i.e. no mutual
//!   exclusion at all. Everything below is scoped to a single `Command`.
//! * **R5 — ambient credentials cannot reach a child, and the machinery is asserted.**
//!   `TOGETHER_API_KEY` exported on the maintainer's machine has already caused a test to make a
//!   real network call; ambient AWS credentials once made an "offline, credential-less" ACP suite
//!   receive a live Bedrock 403; ambient `CYRUP_INTERCOM=1` has leaked 13 broker processes out of
//!   one run. A hand-kept denylist is how each of those happened — the list knew 4 names while
//!   `cyrup_provider::env_api_keys` reads ~45. So this module is built lock-tight instead:
//!
//!   1. **The credential inventory is DERIVED, not copied.** [`provider_keys`] chains
//!      `cyrup_provider::env_api_keys::CREDENTIAL_ENV_VARS` — the slice the provider crate itself
//!      keeps in sync with its own source via the
//!      `credential_env_inventory_covers_every_name_this_file_reads` test — so a provider added
//!      upstream is scrubbed here with no edit in this crate.
//!   2. **Spawned `cyrup` children are hermetic by construction.** [`hermetic`] is `env_clear()`
//!      plus a tiny allowlist, and the [`every_cyrup_spawn_site_is_hermetic`] lint test fails the
//!      suite if any test file wraps `support::bins::cyrup()` in anything else. A denylist can
//!      only remove names someone thought of; `env_clear` removes the names nobody thought of,
//!      including `CYRUP_HOME` — which OUTRANKS the `HOME` a test sets (the home ladder is
//!      `CYRUP_HOME -> HOME -> OS home`, `cyrup-config/src/paths.rs:361`), so an ambient value
//!      would point a child at the developer's REAL `auth.json` with every `*_API_KEY` scrubbed.
//!   3. **The guards are inherited by every target automatically.** The `#[test]`s at the bottom
//!      of this file compile into every `[[test]]` binary that declares `mod support` — which is
//!      all of them except `verify_redaction`, whose single-test soundness argument (`unsafe
//!      set_var` with no concurrent reader) would be destroyed by any added test, and which spawns
//!      only `echo` through the acceptance ledger.
//!
//!   A developer whose shell exports real keys gets a named red from the guards when running the
//!   suite raw, and a green, spend-free run via `cargo run -p xtask -- it`, which re-execs the
//!   suite under `env_clear` + allowlist (the `env -i` shape of pi's own `test.sh`). Both paths
//!   spend zero tokens: the guard reds before any test runs, and the wrapper removes what the
//!   guard would red on. The guard cannot be replaced by the wrapper alone — in-process seams like
//!   `session_svc/model_registry.rs` reach `provider_is_configured(.., env: None)`, which reads
//!   the PROCESS environment non-injectably (1:1 Pi parity), so "no provider is configured
//!   ambiently" must be a property of the harness process itself.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Legacy / alias credential names that real shells export but the provider map does not read.
/// Scrubbed and guarded anyway — an alias on the machine usually means the real one is nearby.
pub const LEGACY_PROVIDER_KEYS: &[&str] = &["TOGETHER_AI_API_KEY"];

/// Provider credentials. Ambient values here make a test spend real tokens.
///
/// Derived from the provider crate's own inventory (see the module doc, point 1) plus
/// [`LEGACY_PROVIDER_KEYS`]. This is a function, not a const, because the whole point is that the
/// authoritative slice lives in `cyrup-provider` and is chained at runtime rather than copied.
pub fn provider_keys() -> impl Iterator<Item = &'static str> {
    cyrup_provider::env_api_keys::CREDENTIAL_ENV_VARS
        .iter()
        .chain(LEGACY_PROVIDER_KEYS)
        .copied()
}

/// Opt-in gates for the three companion subsystems. An ambient `1` here turns a subsystem on
/// underneath a test that assumes it is off — which is how a run leaked 13 broker processes.
pub const FEATURE_GATE_KEYS: &[&str] = &[
    "CYRUP_INTERCOM",
    "CYRUP_SUBAGENTS",
    "CYRUP_PERMISSION_SYSTEM",
];

/// Variables that redirect a child's config/credential RESOLUTION rather than carrying a
/// credential themselves. `CYRUP_HOME` is the dangerous one: it outranks the `HOME` a test sets,
/// so an ambient value re-roots the child at the developer's real `auth.json`/`models.json`.
pub const CONFIG_REDIRECT_KEYS: &[&str] = &["CYRUP_HOME", "CYRUP_AGENT_DIR", "CYRUP_CODING_AGENT_DIR"];

/// Ambient proxies. Not credentials, but they decide where a child's traffic GOES, and a test
/// that asserts "no network" must not have its child silently tunnelled through one.
pub const PROXY_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// Everything [`scrub`] removes: derived credentials, opt-in gates, config redirects, proxies.
pub fn scrubbed_keys() -> impl Iterator<Item = &'static str> {
    provider_keys()
        .chain(FEATURE_GATE_KEYS.iter().copied())
        .chain(CONFIG_REDIRECT_KEYS.iter().copied())
        .chain(PROXY_KEYS.iter().copied())
}

/// Variables reinstated by [`hermetic`] after `env_clear`. Deliberately tiny.
///
/// `PATH` because a child that shells out needs one; `TMPDIR`/`TZ`/`LANG` because their absence
/// changes behaviour in ways unrelated to what any test asserts. `HOME` is NOT here — it is set
/// explicitly to the test's own scratch dir, never inherited. Nothing on this list may ever
/// intersect [`scrubbed_keys`]; [`allowlist_cannot_reinstate_a_scrubbed_name`] pins that.
const ALLOWLIST: &[&str] = &["PATH", "TMPDIR", "TZ", "LANG", "LC_ALL"];

/// Remove the ambient credentials, opt-in gates, config redirects and proxies from a child
/// `Command`, leaving everything else inherited.
///
/// Use this for a child that legitimately needs the developer's environment (a `git`/`cargo`
/// subprocess, say). For a child that is being asserted on — and for ANY spawn of the `cyrup`
/// binary, which the [`every_cyrup_spawn_site_is_hermetic`] lint enforces — use [`hermetic`]:
/// a denylist can only remove names someone thought of.
pub fn scrub(cmd: &mut Command) -> &mut Command {
    for key in scrubbed_keys() {
        cmd.env_remove(key);
    }
    cmd
}

/// A `Command` with **no** inherited environment beyond [`ALLOWLIST`], plus `HOME` pointed at the
/// test's own scratch directory.
///
/// This is the `env -i` + allowlist shape of pi's own `test.sh`, and the only construction that
/// makes "this run cannot reach a real provider" a property of the test rather than of the
/// machine it runs on. It is the REQUIRED constructor for the `cyrup` binary: the
/// [`every_cyrup_spawn_site_is_hermetic`] lint reds any spawn site that wraps
/// `support::bins::cyrup()` in a bare `Command::new` — that shape inherits whatever the
/// developer's shell exports, and each of the incidents in the module doc came through exactly
/// such a site.
pub fn hermetic(program: impl AsRef<OsStr>, home: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    for key in ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.env("HOME", home);
    cmd
}

/// The §4 R5 layer-3 guard: fail loudly if the SUITE'S OWN process carries provider credentials.
///
/// Layers 1 and 2 (hermetic children, injected config) cannot give you this — it is what turns "a
/// test quietly used a real API" into a named red at the top of the run. It runs in every target
/// automatically via the `#[test]`s at the bottom of this file; call it inline as well where an
/// in-process env read is the test's own precondition (`session_svc/model_registry.rs` does).
///
/// A nextest setup script is deliberately not used for this: a setup script can only *append*
/// `KEY=value` lines to `$NEXTEST_ENV`, so it can blank a variable but not unset it — and blanking
/// defeats a value check while still passing an `is_some()` check.
pub fn assert_no_ambient_provider_credentials() {
    let leaked: Vec<&str> = provider_keys()
        .filter(|k| std::env::var_os(k).is_some())
        .collect();
    assert!(
        leaked.is_empty(),
        "ambient provider credentials in the test environment: {leaked:?}. Run the suite through \
         `cargo run -p xtask -- it` (which re-execs it under a cleared environment), or unset \
         them — a test has previously made a REAL network call because TOGETHER_API_KEY was \
         exported, and ambient AWS credentials once turned an offline ACP suite into a live \
         Bedrock 403."
    );
}

/// The same guard for the three opt-in gates, which turn subsystems on underneath a test that
/// assumes they are off.
pub fn assert_no_ambient_feature_gates() {
    let leaked: Vec<&&str> = FEATURE_GATE_KEYS
        .iter()
        .filter(|k| std::env::var_os(k).is_some())
        .collect();
    assert!(
        leaked.is_empty(),
        "ambient opt-in gates in the test environment: {leaked:?}. An ambient CYRUP_INTERCOM has \
         previously leaked 13 broker processes out of one run. `cargo run -p xtask -- it` clears \
         them for the whole suite."
    );
}

// ==================================================================================================
// §4 R5, layer 3 — the guards, compiled into EVERY target that declares `mod support`.
//
// Living here rather than in each `main.rs` is the lock: a new `[[test]]` target cannot forget
// them, because it cannot spawn anything without `support::bins`, and including `support` includes
// these. (`verify_redaction` deliberately has no `mod support`; see the module doc.)
// ==================================================================================================

/// Layer 3a — the harness process itself carries no provider credential (full derived inventory).
#[test]
fn no_ambient_provider_credentials() {
    assert_no_ambient_provider_credentials();
}

/// Layer 3b — the harness process carries none of the subsystem opt-in gates.
#[test]
fn no_ambient_feature_gates() {
    assert_no_ambient_feature_gates();
}

/// Layer 3c — the [`hermetic`] allowlist can never reinstate a name the scrub exists to remove.
/// `HOME` is additionally pinned absent: [`hermetic`] SETS it, it must never inherit it.
#[test]
fn allowlist_cannot_reinstate_a_scrubbed_name() {
    for key in ALLOWLIST {
        assert!(
            !scrubbed_keys().any(|s| s == *key),
            "ALLOWLIST entry {key} is also a scrubbed name — hermetic() would reinstate what \
             scrub() removes"
        );
        assert_ne!(*key, "HOME", "HOME is set explicitly, never inherited");
    }
    // And the derived inventory really is derived: the provider crate's slice is non-trivial and
    // present in full. 40+ is a floor, not a target — it only shrinks if cyrup-provider does.
    assert!(
        cyrup_provider::env_api_keys::CREDENTIAL_ENV_VARS.len() >= 40,
        "cyrup_provider::env_api_keys::CREDENTIAL_ENV_VARS shrank suspiciously"
    );
}

/// Layer 3d — THE SPAWN-SITE LINT. Every place in this suite that names the real `cyrup` binary
/// must build its `Command` with [`hermetic`] (or `Scratch::command`, which composes it).
///
/// The failure mode this rules out is structural, not stylistic: a `Command::new(bins::cyrup())`
/// inherits the developer's whole environment, and the home ladder (`CYRUP_HOME -> HOME`) plus
/// ~45 credential vars mean "inherit + denylist" can never be proven complete. This suite had
/// EIGHT such hand-rolled builders, each with a different denylist, one of which shipped a bug
/// that had to be fixed in four copies separately (`11-cyrup-intercom.md:791`).
///
/// Mechanics: scan every `tests/**/*.rs` line that names `bins::cyrup()` outside a comment; it
/// must have `hermetic(` or `.command(` within the two lines above it or on the same line
/// (rustfmt may wrap the call). `support/bins.rs` (the accessor's own definition/doc) and this
/// file are skipped by name. `CARGO_BIN_EXE_cyrup` is forbidden outright on non-comment lines —
/// it does not compile in this crate, but a copy-paste from a source crate should fail with THIS
/// message, not a mysterious build error.
#[test]
fn every_cyrup_spawn_site_is_hermetic() {
    // Built at runtime so this file's own source cannot satisfy a naive text match.
    let needle: String = ["bins::cyrup", "()"].concat();
    let forbidden: String = ["CARGO_BIN_EXE", "_cyrup"].concat();
    let tests_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");

    let mut sources = Vec::new();
    collect_rs_files(&tests_root, &mut sources);
    assert!(
        sources.len() >= 40,
        "spawn-site lint found only {} .rs files under {} — the scan root is wrong",
        sources.len(),
        tests_root.display()
    );

    let mut violations = Vec::new();
    for path in &sources {
        let rel = path.strip_prefix(&tests_root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str == "support/bins.rs" || rel_str == "support/env.rs" {
            continue;
        }
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if trimmed.contains(&forbidden) {
                violations.push(format!(
                    "{rel_str}:{}: `{forbidden}` does not resolve across packages; use \
                     support::bins::cyrup() through support::env::hermetic()",
                    i + 1
                ));
            }
            if !trimmed.contains(&needle) {
                continue;
            }
            let window_start = i.saturating_sub(2);
            let hermetic_nearby = lines[window_start..=i]
                .iter()
                .any(|l| l.contains("hermetic(") || l.contains(".command("));
            if !hermetic_nearby {
                violations.push(format!(
                    "{rel_str}:{}: `{needle}` outside support::env::hermetic() / \
                     Scratch::command() — this child would inherit the developer's environment \
                     (CYRUP_HOME, ~45 credential vars) and can spend real tokens",
                    i + 1
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "non-hermetic cyrup spawn site(s):\n  {}",
        violations.join("\n  ")
    );
}

/// Depth-first `.rs` collection under `dir`. std-only; the suite has no walkdir dependency.
fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => panic!("read_dir {}: {e}", dir.display()),
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
