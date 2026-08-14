//! Manifest parsing + world-version gating (arch-08 §4.2) and the content-addressed artifact-cache
//! key (arch-08 §4.2 / §6.4, R-ARCH-EXT-016), plus toolchain detection (R-ARCH-EXT-015).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::build::{cache_key, detect_toolchain, world_abi_id, ArtifactCache};
use crate::{ExtensionManifest, HOST_WORLD};

/// `HOST_WORLD` with its MINOR decremented — a guest built against the world one revision back.
/// Derived rather than hard-coded so this stays a real "previous world" after the next bump.
fn one_minor_behind_host() -> String {
    let (pkg, ver) = HOST_WORLD.split_once('@').expect("HOST_WORLD is `name@version`");
    let mut parts = ver.split('.');
    let major = parts.next().unwrap_or("0");
    let minor: u32 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    assert!(minor > 0, "HOST_WORLD minor must be > 0 for a previous world to exist: {HOST_WORLD}");
    format!("{pkg}@{major}.{}", minor - 1)
}

#[test]
fn manifest_parses_camelcase_and_capabilities() {
    let json = format!(
        r#"{{
        "id": "todo-tool",
        "version": "0.2.1",
        "world": "{HOST_WORLD}",
        "entry": "src/lib.rs",
        "capabilities": {{ "fs": ["read:.", "write:.cyrup/todo"], "exec": false, "net": false, "ui": true }}
    }}"#
    );
    let m = ExtensionManifest::from_json(json.as_bytes()).unwrap();
    assert_eq!(m.id, "todo-tool");
    assert_eq!(m.entry.as_deref(), Some("src/lib.rs"));
    assert_eq!(m.capabilities.fs, vec!["read:.", "write:.cyrup/todo"]);
    assert!(m.capabilities.ui);
    assert!(!m.capabilities.net);
}

#[test]
fn world_version_compatible_same_major_and_at_least_the_host_minor() {
    let m = ExtensionManifest {
        id: "x".into(),
        version: "1.0.0".into(),
        world: HOST_WORLD.into(),
        entry: None,
        capabilities: Default::default(),
    };
    assert!(m.check_world(HOST_WORLD).is_ok());

    // A guest built against a NEWER minor is accepted: it may want imports this host lacks, and
    // that failure is specific and reportable. The reverse (below) is not.
    let ahead = ExtensionManifest { world: "cyrup:ext@0.99".into(), ..m.clone() };
    assert!(ahead.check_world(HOST_WORLD).is_ok());
}

/// An EXPORT change — SEAM-005 ADDING `events.on-agent-settled`, EXT-028 RE-SIGNING
/// `events.on-tool-result` — is breaking for an already-built guest: it either lacks the function
/// or has it at the old signature. Without a MINOR check that guest passes `check_world` and then
/// dies at instantiation with an opaque wasmtime link error (`Extension::instantiate_async`
/// resolves the world's exports eagerly); with it, the host refuses up front with a typed
/// `ExtError::WorldVersion` naming both versions.
#[test]
fn world_version_older_minor_is_rejected_before_instantiation() {
    let previous = one_minor_behind_host();
    let m = ExtensionManifest {
        id: "x".into(),
        version: "1.0.0".into(),
        world: previous.clone(),
        entry: None,
        capabilities: Default::default(),
    };
    let err = m.check_world(HOST_WORLD).unwrap_err();
    assert!(
        matches!(&err, crate::ExtError::WorldVersion { found, required }
            if *found == previous && required == HOST_WORLD),
        "a guest one MINOR behind the host is refused with a typed error: {err:?}"
    );
}

/// EXT-028 regression, stated concretely: `f777e44` re-signed `events.on-tool-result` (adding
/// `usage-json`) in both `world.wit` copies and left `HOST_WORLD` at `cyrup:ext@0.2`. A component
/// built before that commit still declares `cyrup:ext@0.2`, so it passed `check_world` — called on
/// the real load path in `facade.rs::load_discovered` — and then failed inside wasmtime instead of
/// producing the typed error the gate exists for. It must now be REFUSED.
#[test]
fn a_guest_built_before_the_on_tool_result_re_signing_is_refused() {
    let m = ExtensionManifest {
        id: "pre-f777e44".into(),
        version: "1.0.0".into(),
        world: "cyrup:ext@0.2".into(),
        entry: None,
        capabilities: Default::default(),
    };
    let err = m.check_world(HOST_WORLD).unwrap_err();
    assert!(
        matches!(&err, crate::ExtError::WorldVersion { found, required }
            if found == "cyrup:ext@0.2" && required == HOST_WORLD),
        "a guest whose `on-tool-result` predates the `usage-json` re-signing is refused: {err:?}"
    );
}

#[test]
fn world_version_incompatible_major_surfaces_error() {
    let m = ExtensionManifest {
        id: "x".into(),
        version: "1.0.0".into(),
        world: "cyrup:ext@2.0".into(),
        entry: None,
        capabilities: Default::default(),
    };
    let err = m.check_world(HOST_WORLD).unwrap_err();
    assert!(matches!(err, crate::ExtError::WorldVersion { .. }));
}

#[test]
fn cache_key_is_deterministic_and_input_sensitive() {
    let src = b"source-tree-hash-bytes";
    let k1 = cache_key(src, "rustc-1.96::wasm32-wasip2", "cyrup:ext@0.1.0");
    let k2 = cache_key(src, "rustc-1.96::wasm32-wasip2", "cyrup:ext@0.1.0");
    assert_eq!(k1, k2, "same inputs -> same key");

    // Any input change busts the cache.
    let k_src = cache_key(b"different", "rustc-1.96::wasm32-wasip2", "cyrup:ext@0.1.0");
    let k_tc = cache_key(src, "rustc-1.97::wasm32-wasip2", "cyrup:ext@0.1.0");
    let k_world = cache_key(src, "rustc-1.96::wasm32-wasip2", "cyrup:ext@0.2.0");
    assert_ne!(k1, k_src);
    assert_ne!(k1, k_tc);
    assert_ne!(k1, k_world);

    assert_eq!(k1.as_str().len(), 64, "blake3 hex is 64 chars");
}

/// EXT-028, the cache half. `hash_source_tree` walks only the authored extension crate directory,
/// so a guest linking `cyrup-ext-sdk` from OUTSIDE that tree used to keep hitting a stale artifact
/// after a `world.wit` or SDK edit. The world identity the build loop keys on must therefore carry
/// BOTH the declared world version and a fingerprint of those out-of-tree ABI sources.
#[test]
fn the_build_loop_keys_on_the_world_identity_not_the_bare_world_string() {
    let id = world_abi_id();
    assert!(id.starts_with(HOST_WORLD), "world identity leads with HOST_WORLD: {id}");
    assert_ne!(id, HOST_WORLD, "the bare world string alone must not be the cache input");

    let fingerprint = id.rsplit("+abi:").next().unwrap_or_default();
    assert_eq!(fingerprint.len(), 64, "the ABI fingerprint is a blake3 hex digest: {id}");
    assert!(
        fingerprint.chars().all(|c| c.is_ascii_hexdigit()),
        "the ABI fingerprint is hex, not the build.rs `unknown` sentinel: {id}"
    );

    // ...and it is genuinely load-bearing: two worlds that differ ONLY in the fingerprint key apart.
    let src = b"source-tree-hash-bytes";
    let tc = "rustc-1.96::wasm32-wasip2";
    assert_ne!(
        cache_key(src, tc, &id),
        cache_key(src, tc, HOST_WORLD),
        "an ABI-source change must bust the artifact cache"
    );
}

#[test]
fn artifact_cache_store_and_hit() {
    let dir = std::env::temp_dir().join(format!("cyrup-ext-cache-test-{}", std::process::id()));
    let cache = ArtifactCache::new(dir.clone());
    let key = cache_key(b"x", "tc", "w");
    assert!(!cache.is_hit(&key));
    let path = cache.store(&key, b"\0asm-component-bytes").unwrap();
    assert!(path.is_file());
    assert!(cache.is_hit(&key), "stored artifact is a cache hit (skips cargo)");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toolchain_detection_reports_status_without_crashing() {
    // Detection must never error/crash; it reports a status and an actionable message on a miss
    // (R-ARCH-EXT-015). We assert the API shape, not a specific host's installation state.
    let tc = detect_toolchain();
    assert_eq!(tc.target, "wasm32-wasip2");
    if !tc.status.is_ready() {
        assert!(tc.status.actionable().is_some(), "a miss must surface an actionable message");
    }
    // toolchain id folds rustc version + target (busts the cache on change).
    assert!(tc.id().contains("wasm32-wasip2"));
}
