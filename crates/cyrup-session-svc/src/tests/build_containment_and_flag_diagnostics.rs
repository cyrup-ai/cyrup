//! EXT-S01 + SEAM-S01 — two build-time failure modes that must be CONTAINED and REPORTED, not
//! swallowed (EXT-S01) or fatal (SEAM-S01).
//!
//! **EXT-S01.** `SessionBuilder::build` loaded its native built-ins with
//!
//! ```ignore
//! for ext in self.native_extensions {
//!     host.load_native_with_services(ext, native_services.clone()).await?;
//! }
//! ```
//!
//! — a bare `?`, so ONE extension's `init()` returning `Err` aborted the WHOLE build: no session at
//! all, and every native after it in the loop was never even attempted. Three built-ins go through
//! that seam in the shipped bin (permission-system, intercom, subagents). Pi records a
//! per-extension load failure and keeps building (`LoadExtensionsResult.errors`, rendered as
//! `Failed to load extension "<path>": <err>` at `main.ts:735-738`).
//!
//! The fix must satisfy BOTH halves — the session builds AND the failure is visible. A silent
//! `let _ =` would pass "the session builds" and be strictly worse than the bug, so every test here
//! asserts the diagnostic too. `StartupDiagnostics::extensions` is the channel the interactive
//! `[Extension issues]` startup panel reads (`cyrup_tui::StartupReport::from_session` →
//! `cyrup_tui::extension_diagnostics`), i.e. the panel added in d2c5509.
//!
//! And the panel is not sufficient on its own, because it is INTERACTIVE-ONLY. Pi's containment
//! ends in an exit: every recorded error becomes `{type:"error"}` on `runtime.diagnostics`
//! (main.ts:735-738) and the bin reports it and exits 1 (main.ts:843-849), in every mode. So the
//! contained failure must ALSO reach `AgentSessionRuntime::diagnostics()` — see the "fatal half"
//! section at the bottom of this file. The one exception is the project-trust skip, which Pi
//! filters out before its loader runs and therefore never treats as a load failure.
//!
//! **SEAM-S01.** A captured CLI `--flag` no loaded extension registered used to be `continue`d away
//! inside the ext-host. Pi turns it into an `{type:"error"}` diagnostic that reaches
//! `runtime.diagnostics` (agent-session-services.ts:98-125, merged at `:182`) and makes the bin
//! exit 1 (`main.ts:843-848`). Asserted here at the `AgentSessionRuntime::diagnostics()` seam the
//! bin's new checkpoint consumes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    AgentSessionRuntime, ExtensionFlagValue, SessionBuilder, SessionConfig, SessionFactory,
    SessionTarget,
};
use cyrup_core::ExtensionId;
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use tempfile::TempDir;

/// A native built-in whose `init()` always fails — the EXT-S01 trigger.
struct FailingExt {
    id: &'static str,
}

#[async_trait::async_trait]
impl NativeExtension for FailingExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Err(ExtError::Panicked(
            "boom: could not open the policy file".to_string(),
        ))
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A native built-in that records whether it was reached — proves the loop KEEPS GOING past the
/// failure instead of stopping at the first error.
struct MarkerExt {
    id: &'static str,
    inited: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl NativeExtension for MarkerExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        self.inited.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

fn faux() -> Arc<dyn Provider> {
    Arc::new(FauxProvider::new())
}

// ================================================================================ EXT-S01 =======

/// THE headline: one native extension's `init()` failure no longer takes the session down, the
/// extensions AFTER it in the load order still load, and the failure is recorded where the startup
/// panel reads it.
#[tokio::test]
async fn a_failing_native_init_is_contained_and_recorded_not_fatal() {
    let fx = fixture();
    let reached = Arc::new(AtomicBool::new(false));

    let session = SessionBuilder::new(faux(), base_config(&fx))
        .with_native_extension(Arc::new(FailingExt {
            id: "permission-system",
        }) as Arc<dyn NativeExtension>)
        .with_native_extension(Arc::new(MarkerExt {
            id: "intercom",
            inited: reached.clone(),
        }) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("a failing native extension must NOT abort the session build");

    // (1) The loop kept going: the extension registered after the failure still initialised.
    assert!(
        reached.load(Ordering::SeqCst),
        "the native load loop stopped at the first failure — later built-ins were never attempted"
    );

    // (2) The failure is SURFACED, not swallowed. This is the `[Extension issues]` channel
    //     `StartupReport::from_session` feeds to `cyrup_tui::extension_diagnostics` (d2c5509).
    let diags = &session.services().startup_diagnostics.extensions;
    assert_eq!(
        diags.len(),
        1,
        "expected exactly one recorded extension failure, got {diags:?}"
    );
    assert_eq!(
        diags[0].path,
        PathBuf::from("permission-system"),
        "the diagnostic must name the extension that failed"
    );
    assert!(
        diags[0]
            .error
            .contains("boom: could not open the policy file"),
        "the diagnostic must carry the underlying error, got {:?}",
        diags[0].error
    );

    // (3) The failed extension is genuinely absent — containment is not "load it anyway".
    let loaded: Vec<String> = session
        .services()
        .ext_host
        .loaded_ids()
        .iter()
        .map(|i| i.to_string())
        .collect();
    assert!(
        !loaded.contains(&"permission-system".to_string()),
        "loaded ids: {loaded:?}"
    );
    assert!(
        loaded.contains(&"intercom".to_string()),
        "loaded ids: {loaded:?}"
    );
}

/// EVERY failing native is recorded, not just the first — the pre-fix `?` could only ever produce
/// one error, because the build died on it.
#[tokio::test]
async fn every_failing_native_is_recorded_independently() {
    let fx = fixture();
    let session = SessionBuilder::new(faux(), base_config(&fx))
        .with_native_extension(Arc::new(FailingExt { id: "alpha" }) as Arc<dyn NativeExtension>)
        .with_native_extension(Arc::new(FailingExt { id: "beta" }) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build");

    let ids: Vec<String> = session
        .services()
        .startup_diagnostics
        .extensions
        .iter()
        .map(|d| d.path.display().to_string())
        .collect();
    assert_eq!(ids, vec!["alpha".to_string(), "beta".to_string()]);
}

/// A clean build still records nothing — containment must not manufacture diagnostics.
#[tokio::test]
async fn a_clean_native_load_records_no_extension_diagnostics() {
    let fx = fixture();
    let reached = Arc::new(AtomicBool::new(false));
    let session = SessionBuilder::new(faux(), base_config(&fx))
        .with_native_extension(Arc::new(MarkerExt {
            id: "ok-ext",
            inited: reached.clone(),
        }) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build");
    assert!(reached.load(Ordering::SeqCst));
    assert!(session.services().startup_diagnostics.extensions.is_empty());
    assert!(session.services().startup_diagnostics.is_empty());
}

// =============================================================================== SEAM-S01 =======

fn config_with_flags(fx: &Fixture, flags: &[(&str, ExtensionFlagValue)]) -> SessionConfig {
    let mut cfg = base_config(fx);
    cfg.extension_flag_values = flags
        .iter()
        .map(|(n, v)| ((*n).to_string(), v.clone()))
        .collect();
    cfg
}

/// A mistyped `--flag` reaches the runtime diagnostics channel as an ERROR — the observable the
/// bin's `report_runtime_diagnostics` checkpoint turns into `Error: …` on stderr + exit 1.
#[tokio::test]
async fn an_unknown_cli_flag_becomes_a_runtime_error_diagnostic() {
    let fx = fixture();
    let cfg = config_with_flags(&fx, &[("no-such-flag", ExtensionFlagValue::Bool(true))]);
    let runtime = AgentSessionRuntime::create(
        Arc::new(SessionFactory::new(faux(), cfg)),
        SessionTarget::New,
    )
    .await
    .expect("build");

    let diags = runtime.diagnostics().await;
    assert_eq!(diags.len(), 1, "expected one diagnostic, got {diags:?}");
    assert_eq!(
        diags[0].severity, "error",
        "Pi types this as an error (exit 1), not a warning"
    );
    assert_eq!(diags[0].message, "Unknown option: --no-such-flag");
    assert_eq!(diags[0].source.as_deref(), Some("extension-flag"));
}

/// The same value is also on the services-level `StartupDiagnostics`, and a build with NO captured
/// flags stays clean (the diagnostic is produced by the reconciliation, not by every build).
#[tokio::test]
async fn no_captured_flags_means_no_flag_diagnostics() {
    let fx = fixture();
    let clean = SessionBuilder::new(faux(), base_config(&fx))
        .build()
        .await
        .expect("build");
    assert!(clean.services().startup_diagnostics.flags.is_empty());

    let cfg = config_with_flags(
        &fx,
        &[
            ("alpha", ExtensionFlagValue::Str("1".into())),
            ("beta", ExtensionFlagValue::Bool(true)),
        ],
    );
    let dirty = SessionBuilder::new(faux(), cfg)
        .build()
        .await
        .expect("build");
    assert_eq!(
        dirty.services().startup_diagnostics.flags,
        vec!["Unknown options: --alpha, --beta".to_string()]
    );
}

// ======================================================= EXT-S01, the FATAL half ================
//
// Containment is only half of Pi's behaviour. Pi contains the failure per-extension
// (`loader.ts:537-540` `errors.push(...); continue`) and THEN refuses to run: every recorded error
// is lifted onto `runtime.diagnostics` as
// `{type:"error", message:'Failed to load extension "<path>": <err>'}` (`main.ts:735-738`) and the
// bin reports it and `process.exit(1)`s (`main.ts:843-849`) — in EVERY mode.
//
// Routing a contained failure to `StartupDiagnostics::extensions` alone is not enough, because the
// only consumer of that field is the interactive `[Extension issues]` panel: under `cyrup -p …`,
// `--mode json` or `--rpc` a failed built-in would produce no message, no diagnostic and exit 0.
// The natives here are cyrup's own security built-ins (the permission gate among them), so silence
// converts a fail-CLOSED abort into a fail-OPEN session.

/// THE headline for the fatal half: the contained failure reaches
/// `AgentSessionRuntime::diagnostics()` — the channel the bin's `report_runtime_diagnostics`
/// checkpoint reads in every mode — as an ERROR carrying Pi's exact message shape.
#[tokio::test]
async fn a_contained_native_failure_is_a_fatal_runtime_diagnostic_in_every_mode() {
    let fx = fixture();
    let reached = Arc::new(AtomicBool::new(false));
    let factory = SessionFactory::new(faux(), base_config(&fx))
        .with_native_extension(Arc::new(FailingExt {
            id: "permission-system",
        }) as Arc<dyn NativeExtension>)
        .with_native_extension(Arc::new(MarkerExt {
            id: "intercom",
            inited: reached.clone(),
        }) as Arc<dyn NativeExtension>);

    let runtime = AgentSessionRuntime::create(Arc::new(factory), SessionTarget::New)
        .await
        .expect("a failing native extension must NOT abort the session build");

    // Containment still holds: the later built-in loaded.
    assert!(
        reached.load(Ordering::SeqCst),
        "the load loop stopped at the first failure"
    );

    let diags = runtime.diagnostics().await;
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == "error").collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one fatal diagnostic, got {diags:?}"
    );
    assert_eq!(
        errors[0].message,
        "Failed to load extension \"permission-system\": extension panicked: boom: could not open the policy file",
        "Pi's message shape is `Failed to load extension \"<path>\": <err>` (main.ts:736-737)"
    );
    assert_eq!(errors[0].source.as_deref(), Some("extension"));
}

/// The panel channel and the fatal channel must BOTH carry it — losing either one is a regression.
/// (`StartupDiagnostics::extensions` feeds `[Extension issues]`; `diagnostics()` feeds the exit.)
#[tokio::test]
async fn the_failure_reaches_the_panel_and_the_exit_channel_together() {
    let fx = fixture();
    let factory =
        SessionFactory::new(faux(), base_config(&fx))
            .with_native_extension(
                Arc::new(FailingExt { id: "subagents" }) as Arc<dyn NativeExtension>
            );
    let runtime = AgentSessionRuntime::create(Arc::new(factory), SessionTarget::New)
        .await
        .expect("build");

    let session = runtime.session().await;
    let panel = &session.services().startup_diagnostics.extensions;
    assert_eq!(panel.len(), 1, "panel channel lost the failure: {panel:?}");
    assert!(
        panel[0].fatal,
        "a native init failure is Pi's fatal load-failure class"
    );

    let fatal = runtime.diagnostics().await.iter().any(|d| {
        d.severity == "error"
            && d.message
                .starts_with("Failed to load extension \"subagents\"")
    });
    assert!(fatal, "exit channel lost the failure");
}

/// The CONTROL: a clean build produces NO error diagnostic, so "extension failures are fatal" can
/// never degrade into "every startup is fatal".
#[tokio::test]
async fn a_clean_build_produces_no_fatal_diagnostic() {
    let fx = fixture();
    let reached = Arc::new(AtomicBool::new(false));
    let factory =
        SessionFactory::new(faux(), base_config(&fx)).with_native_extension(Arc::new(MarkerExt {
            id: "ok-ext",
            inited: reached.clone(),
        })
            as Arc<dyn NativeExtension>);
    let runtime = AgentSessionRuntime::create(Arc::new(factory), SessionTarget::New)
        .await
        .expect("build");
    assert!(reached.load(Ordering::SeqCst));
    assert!(
        runtime.diagnostics().await.is_empty(),
        "{:?}",
        runtime.diagnostics().await
    );
}

/// A corrupt on-disk (wasm) extension in the GLOBAL, pre-trust root is the same fatal class — Pi's
/// `loadExtension` catch arm (`loader.ts:487-491`). This drives the real discovery + load path.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_corrupt_disk_extension_is_fatal_too() {
    let fx = fixture();
    let dir = fx.agent_dir.join("extensions").join("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.wasm"), b"this is not a wasm component").unwrap();

    let mut cfg = base_config(&fx);
    cfg.no_extensions = false; // the global root is only scanned when extensions are enabled
    let runtime = AgentSessionRuntime::create(
        Arc::new(SessionFactory::new(faux(), cfg)),
        SessionTarget::New,
    )
    .await
    .expect("a corrupt extension must not abort the build either");

    let diags = runtime.diagnostics().await;
    let hit = diags
        .iter()
        .find(|d| d.message.contains("Failed to load extension"))
        .unwrap_or_else(|| panic!("no load-failure diagnostic, got {diags:?}"));
    assert_eq!(hit.severity, "error");
    assert!(hit.message.contains("broken"), "{}", hit.message);
}

/// The COUNTER-CASE that keeps the fatal channel honest: a project-local extension skipped because
/// the project is UNTRUSTED is not a load failure. Pi filters untrusted project resources out
/// before `loadExtensions` runs, so it never appears in `errors[]` and never exits 1 — merely
/// opening an untrusted repo must not be fatal. cyrup applies the gate inside the load and records
/// the skip on the same vector, so it is marked non-fatal and stays panel-only.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn an_untrusted_project_extension_is_reported_but_never_fatal() {
    let fx = fixture();
    let dir = fx.cwd.join(".cyrup").join("extensions").join("local");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("local.wasm"), b"this is not a wasm component").unwrap();

    let mut cfg = base_config(&fx);
    cfg.no_extensions = false;
    cfg.trust_override = Some(false); // an untrusted project
    let runtime = AgentSessionRuntime::create(
        Arc::new(SessionFactory::new(faux(), cfg)),
        SessionTarget::New,
    )
    .await
    .expect("build");

    let session = runtime.session().await;
    let panel = &session.services().startup_diagnostics.extensions;
    assert_eq!(
        panel.len(),
        1,
        "the skip must still be reported in the panel: {panel:?}"
    );
    assert!(
        !panel[0].fatal,
        "the trust-gate skip is not Pi's load-failure class"
    );
    assert!(
        panel[0].error.contains("untrusted"),
        "expected the trust-gate error, got {:?}",
        panel[0].error
    );

    let diags = runtime.diagnostics().await;
    assert!(
        !diags.iter().any(|d| d.severity == "error"),
        "an untrusted project must not exit 1; got {diags:?}"
    );
}
