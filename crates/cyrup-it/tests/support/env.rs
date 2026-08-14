//! Child-process environment hygiene.
//!
//! Two hard rules, both from docs/TEST-ARCHITECTURE.md §4:
//!
//! * **R2 — nothing here calls `std::env::set_var`/`remove_var`.** They became `unsafe` in edition
//!   2024 and are unsound in a multithreaded process, which every consolidated test binary is. The
//!   33 per-file `static ENV_MUTATION_LOCK`s in the old corpus were 33 *different* mutexes
//!   guarding one shared environment the moment two files shared a binary — i.e. no mutual
//!   exclusion at all. Everything below is scoped to a single `Command`.
//! * **R5 — ambient credentials are scrubbed, and the scrub is asserted.** `TOGETHER_API_KEY` is
//!   exported on the maintainer's machine and has already caused a test to make a real network
//!   call; ambient `CYRUP_INTERCOM=1` has already leaked 13 broker processes. A denylist alone is
//!   not enough for the credential case — `findInitialModel` will launch on ANY provider whose key
//!   happens to be exported — so [`hermetic`] clears the whole environment and reinstates an
//!   allowlist, the shape `crates/cyrup/tests/auth_credential_print.rs:76` already uses and that
//!   `11-cyrup-intercom.md:791` names as the pattern the other four child builders should copy.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Provider credentials. Ambient values here make a test spend real tokens.
pub const PROVIDER_KEYS: &[&str] = &[
    "TOGETHER_API_KEY",
    "TOGETHER_AI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
];

/// Opt-in gates for the three companion subsystems. An ambient `1` here turns a subsystem on
/// underneath a test that assumes it is off — which is how a run leaked 13 broker processes.
pub const FEATURE_GATE_KEYS: &[&str] = &[
    "CYRUP_INTERCOM",
    "CYRUP_SUBAGENTS",
    "CYRUP_PERMISSION_SYSTEM",
];

/// Everything [`scrub`] removes: credentials plus opt-in gates.
pub const SCRUBBED_KEYS: &[&str] = &[
    "TOGETHER_API_KEY",
    "TOGETHER_AI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "CYRUP_INTERCOM",
    "CYRUP_SUBAGENTS",
    "CYRUP_PERMISSION_SYSTEM",
];

/// Variables reinstated by [`hermetic`] after `env_clear`. Deliberately tiny.
///
/// `PATH` because a child that shells out needs one; `TMPDIR`/`TZ`/`LANG` because their absence
/// changes behaviour in ways unrelated to what any test asserts. `HOME` is NOT here — it is set
/// explicitly to the test's own scratch dir, never inherited.
const ALLOWLIST: &[&str] = &["PATH", "TMPDIR", "TZ", "LANG", "LC_ALL"];

/// Remove the ambient credentials and opt-in gates from a child `Command`, leaving everything else
/// inherited.
///
/// Use this for a child that legitimately needs the developer's environment (a `git`/`cargo`
/// subprocess, say). For a child that is being asserted on, prefer [`hermetic`] — a denylist can
/// only remove names someone thought of.
pub fn scrub(cmd: &mut Command) -> &mut Command {
    for key in SCRUBBED_KEYS {
        cmd.env_remove(key);
    }
    cmd
}

/// A `Command` with **no** inherited environment beyond [`ALLOWLIST`], plus `HOME` pointed at the
/// test's own scratch directory.
///
/// This is the `env -i` + allowlist shape of pi's own `test.sh`, and the only construction that
/// makes "this run cannot reach a real provider" a property of the test rather than of the
/// machine it runs on.
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
/// test quietly used a real API" into a named red at the top of the run. Call it from a `#[test]`
/// in each target:
///
/// ```ignore
/// #[test]
/// fn no_ambient_provider_credentials() {
///     support::env::assert_no_ambient_provider_credentials();
/// }
/// ```
///
/// A nextest setup script is deliberately not used for this: a setup script can only *append*
/// `KEY=value` lines to `$NEXTEST_ENV`, so it can blank a variable but not unset it — and blanking
/// defeats a value check while still passing an `is_some()` check.
pub fn assert_no_ambient_provider_credentials() {
    let leaked: Vec<&&str> = PROVIDER_KEYS
        .iter()
        .filter(|k| std::env::var_os(k).is_some())
        .collect();
    assert!(
        leaked.is_empty(),
        "ambient provider credentials in the test environment: {leaked:?}. Unset them before \
         running the integration suite — a test has previously made a REAL network call because \
         TOGETHER_API_KEY was exported."
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
         previously leaked 13 broker processes out of one run."
    );
}
