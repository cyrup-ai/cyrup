//! Bakes the ABI-source fingerprint (EXT-028) into the host as `CYRUP_EXT_ABI_FINGERPRINT`.
//!
//! The Tier-1 artifact cache keys on (extension-crate source ⊕ toolchain ⊕ world). Neither
//! `world.wit` nor the `cyrup-ext-sdk` guest crate lives inside the extension crate directory, so
//! without this an SDK or WIT edit would leave the key unchanged and a rebuild could silently serve
//! a component built against the old world. See `src/build/abi.rs`, which this file `include!`s so
//! the hash the build script computes and the one `tests/manifest_cache.rs` recomputes are the same
//! code.

include!("src/build/abi.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/build/abi.rs");

    // `crates/` — the parent of this package's directory.
    let crates_dir = match std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .and_then(|d| d.parent().map(std::path::Path::to_path_buf))
    {
        Some(d) => d,
        None => {
            // No manifest dir: emit an explicit sentinel rather than failing the build.
            println!("cargo:rustc-env=CYRUP_EXT_ABI_FINGERPRINT=unknown");
            return;
        }
    };

    // Re-run on any change to a tracked file AND on any add/remove within a tracked directory.
    for root in ABI_SOURCE_ROOTS {
        println!("cargo:rerun-if-changed={}", crates_dir.join(root).display());
    }
    for file in abi_source_files(&crates_dir) {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    println!("cargo:rustc-env=CYRUP_EXT_ABI_FINGERPRINT={}", hash_abi_sources(&crates_dir));
}
