// The ABI-source fingerprint (EXT-028). NOTE: this file is BOTH a crate module (`build::abi`) and
// `include!`d verbatim by `crates/cyrup-ext/build.rs`, so it must stay free of `use` statements
// resolving through `crate::`, of inner (`//!`) doc comments, and of anything beyond `std` +
// `blake3` (which is both a dependency and a build-dependency).
//
// Why it exists: `cache::hash_source_tree` walks ONLY the authored extension crate directory, so an
// edit to `world.wit` or to the `cyrup-ext-sdk` guest crate — both of which a Tier-1 guest compiles
// against but neither of which lives inside that directory — would not move the artifact cache key.
// A rebuild could then serve a component built against the OLD world from cache. Folding a
// compile-time fingerprint of those sources into the key closes that hole.

/// The roots, relative to `crates/`, whose contents define the guest ABI from OUTSIDE any authored
/// extension crate: both on-disk `world.wit` copies and the whole `cyrup-ext-sdk` guest crate.
pub const ABI_SOURCE_ROOTS: &[&str] = &[
    "cyrup-ext/wit",
    "cyrup-ext-sdk/wit",
    "cyrup-ext-sdk/src",
    "cyrup-ext-sdk/Cargo.toml",
];

/// Every ABI source file under [`ABI_SOURCE_ROOTS`], sorted, resolved against the workspace
/// `crates/` directory. A missing root is skipped (a published/vendored build may not ship the
/// guest crate) rather than being an error.
pub fn abi_source_files(crates_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for root in ABI_SOURCE_ROOTS {
        collect_abi_files(&crates_dir.join(root), &mut out);
    }
    out.sort();
    out
}

fn collect_abi_files(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        collect_abi_files(&entry.path(), out);
    }
}

/// BLAKE3 over every ABI source file, folding the `crates/`-relative path, the byte length and the
/// bytes of each — the same normalization [`super::cache::hash_source_tree`] uses, so the digest is
/// stable across checkouts and sensitive to any content, rename or add/remove.
pub fn hash_abi_sources(crates_dir: &std::path::Path) -> String {
    let mut hasher = blake3::Hasher::new();
    for path in abi_source_files(crates_dir) {
        let rel = path.strip_prefix(crates_dir).unwrap_or(&path);
        // Normalize the separator so a Windows checkout hashes to the same digest as a Unix one.
        let rel = rel.to_string_lossy().replace('\\', "/");
        hasher.update(rel.as_bytes());
        hasher.update(b"\x00");
        let Ok(bytes) = std::fs::read(&path) else { continue };
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    hasher.finalize().to_hex().to_string()
}
