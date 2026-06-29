//! Tier-1 build loop end-to-end (arch-08 §6.4; R-08-031). Drives the real `cargo build
//! --target wasm32-wasip2` invocation against an authored extension crate (the bundled
//! `cyrup-ext-sdk`), asserts it yields a valid `cyrup:ext` COMPONENT, and that a second call is a
//! content-addressed cache HIT (no rebuild). Gated on a buildable toolchain (cargo + the wasm
//! target) — the wasm32-wasip2 linker componentizes directly, so no wasm-tools is required.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::build::{build_component_in, detect_toolchain, ArtifactCache};
use std::path::PathBuf;

#[test]
fn tier1_cargo_build_emits_a_component_and_caches() {
    let tc = detect_toolchain();
    if !tc.status.can_build() {
        eprintln!("SKIP tier1 build: toolchain not buildable ({:?})", tc.status);
        return;
    }

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cyrup-ext-sdk");
    assert!(crate_dir.join("Cargo.toml").is_file(), "sdk crate dir: {}", crate_dir.display());

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let cache = ArtifactCache::new(std::env::temp_dir().join(format!("cyrup-ext-tier1-{nanos}")));

    // First call: a cache MISS -> a real cargo build -> a validated component.
    let bytes = build_component_in(&crate_dir, &cache).expect("tier-1 build produces a component");
    assert_eq!(bytes.get(0..4), Some(&b"\0asm"[..]), "wasm preamble");
    assert_eq!(bytes.get(6..8), Some(&[0x01, 0x00][..]), "component layer (not a core module)");

    // Second call: a cache HIT returns identical bytes without rebuilding.
    let again = build_component_in(&crate_dir, &cache).expect("cache hit");
    assert_eq!(bytes, again, "content-addressed cache returns the same artifact");
}
