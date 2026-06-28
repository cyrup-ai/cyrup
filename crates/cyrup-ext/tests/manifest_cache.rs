//! Manifest parsing + world-version gating (arch-08 §4.2) and the content-addressed artifact-cache
//! key (arch-08 §4.2 / §6.4, R-ARCH-EXT-016), plus toolchain detection (R-ARCH-EXT-015).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use cyrup_ext::build::{cache_key, detect_toolchain, ArtifactCache};
use cyrup_ext::{ExtensionManifest, HOST_WORLD};

#[test]
fn manifest_parses_camelcase_and_capabilities() {
    let json = br#"{
        "id": "todo-tool",
        "version": "0.2.1",
        "world": "cyrup:ext@0.1",
        "entry": "src/lib.rs",
        "capabilities": { "fs": ["read:.", "write:.cyrup/todo"], "exec": false, "net": false, "ui": true }
    }"#;
    let m = ExtensionManifest::from_json(json).unwrap();
    assert_eq!(m.id, "todo-tool");
    assert_eq!(m.entry.as_deref(), Some("src/lib.rs"));
    assert_eq!(m.capabilities.fs, vec!["read:.", "write:.cyrup/todo"]);
    assert!(m.capabilities.ui);
    assert!(!m.capabilities.net);
}

#[test]
fn world_version_compatible_same_major() {
    let m = ExtensionManifest {
        id: "x".into(),
        version: "1.0.0".into(),
        world: "cyrup:ext@0.1".into(),
        entry: None,
        capabilities: Default::default(),
    };
    assert!(m.check_world(HOST_WORLD).is_ok());
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
    assert!(matches!(err, cyrup_ext::ExtError::WorldVersion { .. }));
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
