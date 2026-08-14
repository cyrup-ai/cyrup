//! gap-07 #2 (the SEAM half of the old `crates/cyrup-session-svc/tests/install_noop.rs`): a
//! package's DECLARED EXTENSION must actually LOAD into the running session — not merely be
//! collected into `ext_crate_paths` and dropped.
//!
//! The file this came from held two tests and split at the module boundary
//! (docs/TEST-ARCHITECTURE.md §3.1): test 1,
//! `installed_global_package_resources_load_in_assembled_session`, asserts in-process over a real
//! `SessionBuilder` and needs no wasm guest at all, so it moved to
//! `crates/cyrup-session-svc/src/tests/install_noop.rs`. Only the `mod wasm_ext` half is a seam —
//! it drives a REAL `wasm32-wasip2` component — and only that half is here. The helpers the two
//! halves shared (`write`, `Fx`, `fixture`) are small and are now duplicated across the split
//! rather than exported; the parent doc of that file records the same.
//!
//! ORIGINAL DOC COMMENT of the module, preserved verbatim:
//!
//! > gap-07 #2: a package's DECLARED EXTENSION must actually LOAD into the running session (not
//! > merely be collected into `ext_crate_paths` and dropped). This drives the REAL assembled session
//! > over a prebuilt wasm component and asserts the guest's `/greet` command is registered in the
//! > live host — end-to-end proof the package-tier extension path is wired into the loader (Pi
//! > `mergePaths(cliEnabledExtensions, enabledExtensions)` → `loadFinalExtensionSet`,
//! > resource-loader.ts:403-407). It ALSO exercises the `package_global_dir` base fix: the package's
//! > working tree is materialized at the exact Global-scope store path `install` uses
//! > (`<package_dir>/packages/<id>`, one level deeper than a naive `<agent_dir>/packages/<id>`), so a
//! > wrong base would fail to resolve the manifest and the extension would never load.
//!
//! The module's original `#[cfg(feature = "wasm-host")]` is deliberately gone: it named
//! cyrup-session-svc's own feature, which that crate enables in its `default`, so it was always
//! true. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which `--features it` does
//! not enable — and every test here would SILENTLY not compile in. See the `[[test]]` note in
//! crates/cyrup-it/Cargo.toml.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_resources::package::lock;
use cyrup_resources::{
    InstallScope, InstalledPackage, InstalledPackages, PackageSource, PackageStore, PinRef,
};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): the `fixture_component()` this module carried shelled
// out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    /// The bin's default `dirs.package_dir` = `<agent_dir>/packages` (env.rs:156-160).
    package_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let package_dir = agent_dir.join("packages");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx { _tmp: tmp, cwd, agent_dir, package_dir }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn package_declared_extension_loads_into_assembled_session() {
    let bytes = bins::component_bytes();
    let fx = fixture();

    // Replicate the post-`install` on-disk state of a GLOBAL git package WITHOUT a real clone
    // (same technique as cyrup-resources' a09_5_update_skips_pinned): write the registry record
    // via `lock::save` and materialize the working tree at the exact store path.
    let store = PackageStore::new(fx.package_dir.clone(), Some(fx.cwd.clone()));
    let source = PackageSource::Git { url: "file:///fake/deploypkg".into(), reff: PinRef::Default };
    let id = source.package_id();
    let pkg_dir = store.package_dir(InstallScope::Global, &id).expect("global package dir");
    // A package whose manifest declares one extension = the demo component (registers `/greet`).
    std::fs::create_dir_all(pkg_dir.join("extensions/demo")).unwrap();
    std::fs::write(pkg_dir.join("extensions/demo/demo.wasm"), &bytes).unwrap();
    write(
        &pkg_dir.join("cyrup.toml"),
        "[package]\nname = \"deploypkg\"\nversion = \"0.1.0\"\n\n\
         [resources]\nextensions = [\"./extensions/demo\"]\n",
    );
    let reg = InstalledPackages {
        packages: vec![InstalledPackage {
            id,
            source,
            scope: InstallScope::Global,
            resolved_commit: Some("deadbeef".into()),
            installed_at: "0".into(),
            disabled: Default::default(),
        }],
    };
    lock::save(&store.registry_path(InstallScope::Global).unwrap(), &reg).unwrap();

    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    // The declared extension actually LOADED: its guest `init` registered `/greet` in the live
    // host command registry (pre-fix, `ext_crate_paths` was collected but never handed to the
    // loader, so `greet` was absent).
    let commands = session.services().ext_host.registry().command_names().unwrap();
    assert!(
        commands.iter().any(|n| n == "greet"),
        "installed package's declared extension must load + register /greet: {commands:?}"
    );
    assert!(
        session.resources().ext_crate_paths.iter().any(|p| p.ends_with("demo")),
        "the package's extension dir is collected in the resource registry"
    );
}
