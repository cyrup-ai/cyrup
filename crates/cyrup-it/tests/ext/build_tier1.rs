//! Tier-1 build loop end-to-end (arch-08 §6.4; R-08-031). Drives the real `cargo build
//! --target wasm32-wasip2` invocation against an authored extension crate (the bundled
//! `cyrup-ext-sdk`), asserts it yields a valid `cyrup:ext` COMPONENT, and that a second call is a
//! content-addressed cache HIT (no rebuild). Gated on a buildable toolchain (cargo + the wasm
//! target) — the wasm32-wasip2 linker componentizes directly, so no wasm-tools is required.
//!
//! MIGRATION NOTE — the one module in this target that keeps a nested `cargo build`. Everywhere
//! else in `tests/ext/` the nested wasip2 build was fixture scaffolding and was replaced by
//! `support::bins::component()`. Here it is the SUBJECT: `build_component_in` is production code
//! (`cyrup_ext::build`) and the assertions are about the build loop itself — that it emits a
//! component, and that a second call hits the content-addressed cache instead of rebuilding.
//! Handing it a prebuilt artifact would delete the test. It already writes its cache to its own
//! `TempDir`, so it does not contend for the workspace build lock.
//!
//! `env!("CARGO_MANIFEST_DIR")).join("../cyrup-ext-sdk")` below still resolves: `crates/cyrup-it`
//! and `crates/cyrup-ext` are siblings, so the relative path is unchanged by the move.
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

    // The cache directory must OUTLIVE both `build_component_in` calls below (the second one is the
    // cache-hit assertion) but must not outlive the test. A `TempDir` gives exactly that: a unique
    // directory removed when `_cache_dir` drops at the end of the function.
    //
    // This was previously a nanos-suffixed path under `std::env::temp_dir()` with no cleanup, so
    // every run of this test leaked its ~213 MB wasm build cache. 57 of them accumulated here and
    // filled a 16 GB `/tmp` tmpfs, at which point `ld` began failing with SIGBUS while linking
    // unrelated doctests — a green suite turning red for reasons nowhere near the change under test.
    let cache_dir = tempfile::Builder::new()
        .prefix("cyrup-ext-tier1-")
        .tempdir()
        .expect("a temp dir for the tier-1 artifact cache");
    let cache = ArtifactCache::new(cache_dir.path().to_path_buf());

    // First call: a cache MISS -> a real cargo build -> a validated component.
    let bytes = build_component_in(&crate_dir, &cache).expect("tier-1 build produces a component");
    assert_eq!(bytes.get(0..4), Some(&b"\0asm"[..]), "wasm preamble");
    assert_eq!(bytes.get(6..8), Some(&[0x01, 0x00][..]), "component layer (not a core module)");

    // Second call: a cache HIT returns identical bytes without rebuilding.
    let again = build_component_in(&crate_dir, &cache).expect("cache hit");
    assert_eq!(bytes, again, "content-addressed cache returns the same artifact");
}
