//! The `cyrup:ext` WIT world has TWO on-disk copies — `crates/cyrup-ext/wit/world.wit` (consumed by
//! the host's `wasmtime::component::bindgen!`) and `crates/cyrup-ext-sdk/wit/world.wit` (consumed by
//! the guest's `wit-bindgen`). Nothing in the build enforces that they agree: if they drift, the host
//! links against one shape and the guest exports another, and the failure surfaces as a raw wasmtime
//! instantiation error at test time rather than a compile error.
//!
//! This is that enforcement — and, since EXT-028, the enforcement of the WORLD VERSION too. Comparing
//! the two copies to each other proves nothing about versions: `f777e44` RE-SIGNED the
//! `events.on-tool-result` export (adding `usage-json`) in BOTH copies, byte-identically, without
//! bumping `HOST_WORLD`. That left a pre-`f777e44` guest declaring the still-current `cyrup:ext@0.2`
//! passing `ExtensionManifest::check_world` and then dying inside wasmtime with an opaque link error
//! — exactly the failure the version gate exists to turn into a typed `ExtError::WorldVersion`.
//! So the tests below tie `HOST_WORLD` to the `package cyrup:ext@…` line of both copies, and tie the
//! header's event-count claim to the exports actually declared.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

fn host_wit() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit/world.wit")
}

fn guest_wit() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cyrup-ext-sdk/wit/world.wit")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Extract the `package cyrup:ext@MAJOR.MINOR.PATCH;` declaration.
fn package_version(src: &str, path: &Path) -> String {
    src.lines()
        .find_map(|l| Some(l.trim().strip_prefix("package ")?.trim_end_matches(';').trim()))
        .unwrap_or_else(|| panic!("no `package ...;` line in {}", path.display()))
        .to_string()
}

#[test]
fn the_host_and_guest_wit_world_copies_are_identical() {
    let host = host_wit();
    let guest = guest_wit();
    let host_src = read(&host);
    let guest_src = read(&guest);

    if host_src != guest_src {
        let first_diff = host_src
            .lines()
            .zip(guest_src.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("line {}:\n  host : {a}\n  guest: {b}", i + 1))
            .unwrap_or_else(|| {
                format!(
                    "line counts differ: host {} vs guest {}",
                    host_src.lines().count(),
                    guest_src.lines().count()
                )
            });
        panic!(
            "the host and guest WIT world copies have drifted — change BOTH:\n  {}\n  {}\n{first_diff}",
            host.display(),
            guest.display()
        );
    }
}

/// EXT-028, the durable half: `HOST_WORLD` and the `package` line move TOGETHER, in BOTH copies.
///
/// `HOST_WORLD` is `cyrup:ext@MAJOR.MINOR`; the WIT package line carries a full semver
/// `cyrup:ext@MAJOR.MINOR.PATCH`. The gate compares MAJOR+MINOR, so those are what must agree.
#[test]
fn host_world_matches_the_wit_package_version_in_both_copies() {
    for path in [host_wit(), guest_wit()] {
        let declared = package_version(&read(&path), &path);
        let (pkg, ver) = declared
            .split_once('@')
            .unwrap_or_else(|| panic!("`package {declared};` is not `name@version` in {}", path.display()));
        let mut parts = ver.split('.');
        let major = parts.next().unwrap_or("");
        let minor = parts.next().unwrap_or("");
        let major_minor = format!("{pkg}@{major}.{minor}");

        assert_eq!(
            major_minor,
            crate::HOST_WORLD,
            "{} declares `package {declared};` but the host gate is {} — ANY change to an EXPORT \
             (added, removed, or RE-SIGNED) must bump BOTH, or a guest built against the old world \
             passes `check_world` and then fails inside wasmtime with a raw link error (EXT-028)",
            path.display(),
            crate::HOST_WORLD,
        );
    }
}

/// EXT-028, the header half: the `// … exports N `on-*` event hooks` claim in the world's own
/// preamble is checked against the exports actually declared, so it cannot rot the way the old
/// "30-event catalog" line did (the `events` interface has long declared 31).
#[test]
fn the_header_event_count_matches_the_declared_event_exports() {
    let path = host_wit();
    let src = read(&path);

    // The `events` interface body, from its opening line to the first column-0 `}`.
    let body: String = src
        .lines()
        .skip_while(|l| !l.starts_with("interface events {"))
        .take_while(|l| *l != "}")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!body.is_empty(), "no `interface events {{` block in {}", path.display());

    let declared = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            l.starts_with("  ") && t.starts_with("on-") && t.contains(':')
        })
        .count();

    let claimed: usize = src
        .lines()
        .find_map(|l| {
            let (_, rest) = l.split_once("interface exports ")?;
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or_else(|| panic!("no `The `events` interface exports N …` claim in {}", path.display()));

    assert_eq!(
        claimed, declared,
        "the world header claims {claimed} `on-*` event exports but {} declares {declared}",
        path.display(),
    );
}

/// EXT-028, the cache half: the Tier-1 artifact key folds a compile-time fingerprint of the ABI
/// sources that `hash_source_tree` cannot see — both `world.wit` copies and the `cyrup-ext-sdk`
/// guest crate. Recomputing it from the same files with the same hasher proves the baked-in value
/// actually tracks them; before EXT-028 there was no fingerprint at all and an SDK/WIT edit left the
/// key untouched, so a rebuild could serve a component built against the old world from cache.
#[test]
fn the_cache_key_tracks_the_wit_and_sdk_sources_outside_the_extension_crate() {
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of cyrup-ext/")
        .to_path_buf();

    let files = crate::build::abi::abi_source_files(&crates_dir);
    for expected in ["cyrup-ext/wit/world.wit", "cyrup-ext-sdk/wit/world.wit"] {
        assert!(
            files.iter().any(|f| f.ends_with(expected)),
            "{expected} is an ABI source and must be fingerprinted; got {files:?}"
        );
    }
    assert!(
        files.iter().any(|f| f.ends_with("cyrup-ext-sdk/src/guest.rs")),
        "the cyrup-ext-sdk guest crate must be fingerprinted; got {files:?}"
    );

    let recomputed = crate::build::abi::hash_abi_sources(&crates_dir);
    assert_eq!(recomputed.len(), 64, "blake3 hex is 64 chars");
    assert_eq!(
        crate::build::ABI_FINGERPRINT,
        recomputed,
        "the ABI fingerprint baked in by build.rs is stale — an edit to a `world.wit` copy or to \
         cyrup-ext-sdk did not bust the Tier-1 artifact cache (EXT-028)"
    );

    let id = crate::build::world_abi_id();
    assert!(id.starts_with(crate::HOST_WORLD), "the world identity leads with HOST_WORLD: {id}");
    assert!(id.ends_with(&recomputed), "the world identity carries the ABI fingerprint: {id}");
}
