//! The project-trust gate must be the FIRST thing `load_discovered` does.
//!
//! `ExtensionHost::load_discovered` used to run `manifest.check_world(HOST_WORLD)` above the trust
//! gate. An untrusted project-local extension whose manifest declared a stale world therefore came
//! back as `ExtError::WorldVersion` rather than `ExtError::Untrusted`, and
//! `discover_and_load` classifies everything except `Untrusted` as `fatal: true`
//! (`facade.rs`, `fatal: !matches!(e, ExtError::Untrusted)`), which
//! `cyrup-session-svc/src/runtime.rs:128-138` renders as a `runtime.diagnostics` error that exits
//! the bin 1. Opening an untrusted project that merely CONTAINS an out-of-date extension aborted
//! startup.
//!
//! pi cannot reach that state: `loadProjectTrustExtensions` forces
//! `settingsManager.setProjectTrusted(false)` and reloads before the extension set is resolved
//! (`core/resource-loader.ts:379-384` @v0.83.0), so project-local extensions are not in the set that
//! `loadExtensions` inspects — they are never parsed, never version-checked, and never diagnosed.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::host::DenyServices;
use crate::loader::{DiscoveredExtension, ExtOrigin};
use crate::{ExtError, ExtensionHost, ExtensionManifest, HostConfig};
use std::sync::Arc;

/// A manifest declaring a world this host does NOT implement (major 0, minor 1 — below
/// `HOST_WORLD`'s minor, which is what `check_world` refuses).
fn stale_world_manifest() -> ExtensionManifest {
    ExtensionManifest {
        id: "stale".into(),
        version: "0.0.1".into(),
        world: "cyrup:ext@0.1".into(),
        entry: None,
        capabilities: crate::Capabilities::none(),
    }
}

fn discovered(origin: ExtOrigin) -> DiscoveredExtension {
    DiscoveredExtension {
        // Deliberately a path that does not exist: nothing below the gate may touch the disk for an
        // untrusted extension, and the pre-trust half of this test must fail on the WORLD, not on a
        // missing artifact — `check_world` runs before `resolve_component_bytes`.
        dir: std::path::PathBuf::from("/nonexistent/cyrup-trust-gate-order"),
        manifest: stale_world_manifest(),
        wasm: None,
        origin,
    }
}

/// PRESENCE first, so the absence assertion below cannot pass vacuously: a PRE-TRUST extension with
/// the same stale manifest really does reach `check_world` and really is refused by it.
#[tokio::test]
async fn a_pre_trust_extension_with_a_stale_world_is_refused_by_check_world() {
    let host = ExtensionHost::new(HostConfig::default());
    let err = host
        .load_discovered(&discovered(ExtOrigin::Global), false, Arc::new(DenyServices))
        .await
        .expect_err("a stale world is refused");
    assert!(
        matches!(err, ExtError::WorldVersion { .. }),
        "global (pre-trust) origin loads regardless of project trust, so the world check decides: \
         {err:?}"
    );
}

/// The gate itself: an untrusted PROJECT-local extension is `Untrusted` even when its manifest
/// would also have failed the world check — the non-fatal class, so startup is not aborted.
#[tokio::test]
async fn an_untrusted_project_extension_is_untrusted_not_a_world_mismatch() {
    let host = ExtensionHost::new(HostConfig::default());
    let err = host
        .load_discovered(&discovered(ExtOrigin::Project), false, Arc::new(DenyServices))
        .await
        .expect_err("an untrusted project-local extension does not load");
    assert!(
        matches!(err, ExtError::Untrusted),
        "the trust gate must decide BEFORE the world check — anything else is classified \
         `fatal: true` and exits the bin 1 for merely opening an untrusted project: {err:?}"
    );
}

/// And trusting the project puts the same extension back on the world-check path — the gate opens,
/// it does not swallow.
#[tokio::test]
async fn trusting_the_project_hands_the_same_extension_to_check_world() {
    let host = ExtensionHost::new(HostConfig::default());
    let err = host
        .load_discovered(&discovered(ExtOrigin::Project), true, Arc::new(DenyServices))
        .await
        .expect_err("a stale world is still refused once the project is trusted");
    assert!(
        matches!(err, ExtError::WorldVersion { .. }),
        "trusted project-local extension is judged on its world like any other: {err:?}"
    );
}
