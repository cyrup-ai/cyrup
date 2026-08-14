//! EXT-028's cache half, as an OBSERVATION rather than an argument: an edit to EITHER on-disk
//! `world.wit` copy must move `CYRUP_EXT_ABI_FINGERPRINT`, and a moved fingerprint must turn a
//! stored Tier-1 artifact into a cache MISS.
//!
//! WHY THIS FILE EXISTS. `crates/cyrup-ext/build.rs` bakes a BLAKE3 of the ABI sources that
//! `cache::hash_source_tree` cannot see — both `world.wit` copies and the whole `cyrup-ext-sdk`
//! guest crate, none of which live inside an authored extension crate directory — into
//! `build::ABI_FINGERPRINT`, and `build::world_abi_id()` folds it into every Tier-1 cache key. The
//! only existing coverage (`cyrup-ext/src/tests/wit_world_sync.rs:141-172`) RECOMPUTES the hash and
//! compares it to the baked-in constant. That proves the constant is current; it proves nothing
//! about INVALIDATION, because it never changes an input and never looks at a cache key. Sweep 2
//! recorded exactly that gap: "the ABI fingerprint in `build.rs` should have invalidated the
//! artifact cache for both WIT copies, but that invalidation is itself untested."
//!
//! The three properties below are the whole chain, and each is asserted against a real filesystem:
//!
//!   1. build.rs's tracked file set — the same list it emits as `cargo:rerun-if-changed` — contains
//!      BOTH `world.wit` copies. Without this, cargo never re-runs the script and the fingerprint
//!      is frozen no matter what the other two properties say.
//!   2. A one-byte edit to EITHER copy moves the fingerprint, and the two copies are
//!      distinguishable (the same edit in the other copy yields a DIFFERENT digest, because the
//!      `crates/`-relative path is folded in alongside the bytes).
//!   3. A moved fingerprint moves the Tier-1 cache key, so a stored artifact stops being a hit.
//!
//! Property 2 runs against a synthetic `crates/`-shaped tree, not the repo: mutating the real
//! `world.wit` would race every other test in this binary and dirty the working tree.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_ext::HOST_WORLD;
use cyrup_ext::build::abi::{ABI_SOURCE_ROOTS, abi_source_files, hash_abi_sources};
use cyrup_ext::build::{ABI_FINGERPRINT, ArtifactCache, cache_key, world_abi_id};
use std::path::{Path, PathBuf};

/// `crates/`, the directory `build.rs` resolves as the parent of its own `CARGO_MANIFEST_DIR`.
/// `crates/cyrup-it` and `crates/cyrup-ext` are siblings, so this is the same directory.
fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of cyrup-it/")
        .to_path_buf()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// A minimal `crates/`-shaped tree carrying one file under each of the four [`ABI_SOURCE_ROOTS`].
fn synthetic_crates_tree() -> tempfile::TempDir {
    let td = tempfile::Builder::new()
        .prefix("cyrup-abi-fingerprint-")
        .tempdir()
        .expect("a temp dir for the synthetic crates tree");
    let root = td.path();
    write(&root.join("cyrup-ext/wit/world.wit"), "package cyrup:ext@0.6.0;\n");
    write(&root.join("cyrup-ext-sdk/wit/world.wit"), "package cyrup:ext@0.6.0;\n");
    write(&root.join("cyrup-ext-sdk/Cargo.toml"), "[package]\nname = \"cyrup-ext-sdk\"\n");
    write(&root.join("cyrup-ext-sdk/src/guest.rs"), "// guest\n");
    td
}

/// Property 1 — build.rs tracks BOTH copies.
///
/// `crates/cyrup-ext/build.rs:30-35` emits one `cargo:rerun-if-changed` per [`ABI_SOURCE_ROOTS`]
/// entry AND one per file [`abi_source_files`] returns; this asserts on that exact list. If a copy
/// falls out of it, cargo never re-runs the script after that copy is edited, and every downstream
/// property here is vacuous — the fingerprint simply never moves.
#[test]
fn build_rs_tracks_both_on_disk_world_wit_copies() {
    let crates = crates_dir();
    let tracked = abi_source_files(&crates);

    for copy in ["cyrup-ext/wit/world.wit", "cyrup-ext-sdk/wit/world.wit"] {
        let expected = crates.join(copy);
        assert!(
            expected.is_file(),
            "{} must exist — the two copies are the host's `bindgen!` input and the guest's \
             `wit-bindgen` input",
            expected.display()
        );
        assert!(
            tracked.iter().any(|f| f == &expected),
            "{copy} is not in build.rs's rerun-if-changed set, so editing it would leave \
             ABI_FINGERPRINT frozen and serve a component built against the OLD world from cache \
             (EXT-028); tracked: {tracked:?}"
        );
    }

    // The roots themselves are what makes an ADDED file inside either `wit/` directory visible.
    for root in ["cyrup-ext/wit", "cyrup-ext-sdk/wit"] {
        assert!(
            ABI_SOURCE_ROOTS.contains(&root),
            "{root} must be an ABI source root; got {ABI_SOURCE_ROOTS:?}"
        );
    }

    // The sentinel `build.rs:24` emits when it cannot resolve `crates/` at all. If this ever fires,
    // the fingerprint is a CONSTANT and the cache key stops tracking the world entirely — which
    // would look exactly like a healthy build.
    assert_ne!(
        ABI_FINGERPRINT, "unknown",
        "build.rs failed to resolve the workspace crates/ dir; the ABI fingerprint is inert"
    );
    assert_eq!(ABI_FINGERPRINT.len(), 64, "blake3 hex is 64 chars: {ABI_FINGERPRINT}");
}

/// Property 2 — an edit to EITHER copy moves the fingerprint, and the copies are distinguishable.
///
/// The second half is the one a naive implementation gets wrong: hashing only file CONTENTS would
/// make "the byte changed in the host copy" and "the same byte changed in the guest copy" collide,
/// so a swap of the two files would be invisible. `hash_abi_sources` folds the `crates/`-relative
/// path and the byte length ahead of the bytes (`build/abi.rs:47-60`), which is what separates them.
#[test]
fn editing_either_world_wit_copy_moves_the_abi_fingerprint() {
    let tree = synthetic_crates_tree();
    let root = tree.path();
    let host_copy = root.join("cyrup-ext/wit/world.wit");
    let guest_copy = root.join("cyrup-ext-sdk/wit/world.wit");
    let original = std::fs::read_to_string(&host_copy).expect("read the synthetic host copy");

    let baseline = hash_abi_sources(root);
    assert_eq!(baseline.len(), 64, "blake3 hex is 64 chars");
    assert_eq!(
        hash_abi_sources(root),
        baseline,
        "the digest must be deterministic over an unchanged tree, or every rebuild is a miss"
    );

    // The exact shape of the 0.5 -> 0.6 bump: the `package` line moves.
    let bumped = "package cyrup:ext@0.7.0;\n";

    write(&host_copy, bumped);
    let after_host_edit = hash_abi_sources(root);
    assert_ne!(
        after_host_edit, baseline,
        "editing crates/cyrup-ext/wit/world.wit left the ABI fingerprint unchanged (EXT-028)"
    );

    write(&host_copy, &original);
    assert_eq!(
        hash_abi_sources(root),
        baseline,
        "restoring the byte must restore the digest — otherwise the hash is order- or \
         timestamp-sensitive and every build is a spurious miss"
    );

    write(&guest_copy, bumped);
    let after_guest_edit = hash_abi_sources(root);
    assert_ne!(
        after_guest_edit, baseline,
        "editing crates/cyrup-ext-sdk/wit/world.wit left the ABI fingerprint unchanged (EXT-028)"
    );
    assert_ne!(
        after_guest_edit, after_host_edit,
        "the SAME edit in the two copies must hash differently — the relative path is folded in, \
         so the host copy and the guest copy are not interchangeable"
    );
}

/// Property 3 — a moved fingerprint turns a stored Tier-1 artifact into a MISS.
///
/// `build_component_in` computes its key as `cache_key(hash_source_tree(crate_dir),
/// toolchain.id(), world_abi_id())` (`build/mod.rs:53-61`) and returns the stored bytes whenever
/// `cache.is_hit(&key)`. Holding the first two inputs fixed and moving only the ABI fingerprint is
/// exactly the scenario EXT-028 was filed for: the authored extension crate did not change, the
/// toolchain did not change, and the WORLD did — so the cache must not answer.
#[test]
fn a_moved_abi_fingerprint_turns_a_stored_tier1_artifact_into_a_cache_miss() {
    // Pin the composition `world_abi_id` uses, so the two ids built below are the real shape and
    // not a private invention of this test.
    assert_eq!(
        world_abi_id(),
        format!("{HOST_WORLD}+abi:{ABI_FINGERPRINT}"),
        "world_abi_id is `<HOST_WORLD>+abi:<fingerprint>` (build/mod.rs:32)"
    );

    // Two fingerprints that differ ONLY because a `world.wit` copy was edited (property 2's tree).
    let tree = synthetic_crates_tree();
    let before = hash_abi_sources(tree.path());
    write(&tree.path().join("cyrup-ext/wit/world.wit"), "package cyrup:ext@0.7.0;\n");
    let after = hash_abi_sources(tree.path());
    assert_ne!(before, after);

    let cache_dir = tempfile::Builder::new()
        .prefix("cyrup-abi-cache-")
        .tempdir()
        .expect("a temp dir for the artifact cache");
    let cache = ArtifactCache::new(cache_dir.path().to_path_buf());

    // The other two key inputs, held FIXED across the two keys: an unchanged extension crate and an
    // unchanged toolchain.
    let source_tree_hash = [0x5a_u8; 32];
    let toolchain_id = "cargo 1.96.0 wasm32-wasip2";

    let key_before = cache_key(&source_tree_hash, toolchain_id, &format!("{HOST_WORLD}+abi:{before}"));
    let key_after = cache_key(&source_tree_hash, toolchain_id, &format!("{HOST_WORLD}+abi:{after}"));
    assert_ne!(
        key_before, key_after,
        "the world identity is folded into the cache key; a `world.wit` edit must move it"
    );

    // A component built against the OLD world, stored under the OLD key. The preamble is the one
    // `build::validate_component` accepts, so this is a plausible stored artifact and not a blob.
    cache
        .store(&key_before, &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00])
        .expect("store the pre-edit artifact");

    // Assert the PRESENCE before asserting the absence: an `is_hit` that is false because nothing
    // was ever written would pass this test while proving nothing.
    assert!(cache.is_hit(&key_before), "the pre-edit artifact is a hit under its own key");
    assert!(
        !cache.is_hit(&key_after),
        "a component built against the OLD world was served from cache after a world.wit edit \
         (EXT-028) — {} exists",
        cache.artifact_for(&key_after).display()
    );
}
