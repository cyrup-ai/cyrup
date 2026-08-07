//! Pi's "direct file" discovery rule for prebuilt components (arch-08 §6.2; Pi
//! `discoverExtensionsInDir`, `loader.ts:628-666` rule 1 "Direct files", and the non-directory
//! configured-path fall-through at `loader.ts:704-717`).
//!
//! A bare artifact dropped straight into a discovery root — `<cwd>/.cyrup/extensions/mytool.wasm`,
//! `<agentDir>/extensions/mytool.wasm`, or `cyrup --extension ./mytool.wasm` — must be discovered.
//! Before the fix `scan_dir` only ever considered directory entries, so every one of these was
//! silently skipped and the extension simply never existed as far as the session was concerned.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_ext::loader::{discover, DiscoveredExtension, DiscoveryRoots, ExtOrigin};
use std::path::{Path, PathBuf};

/// A stand-in component artifact. Discovery never validates the bytes (the wasm header is here only
/// so the file is not obviously absurd); the load path is exercised separately below with the trust
/// gate, which fires before any parsing.
const ARTIFACT: &[u8] = b"\0asm\x0d\x00\x01\x00";

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("cyrup-ext-directfile-{tag}-{nanos}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_artifact(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, ARTIFACT).unwrap();
    p
}

fn find<'a>(found: &'a [DiscoveredExtension], id: &str) -> Option<&'a DiscoveredExtension> {
    found.iter().find(|d| d.manifest.id == id)
}

/// Rule 1 in every root: a manifest-less `*.wasm` sitting directly in the extensions dir is an
/// extension whose id is the artifact stem, and whose origin is the root it came from.
#[test]
fn bare_artifact_directly_in_each_discovery_root_is_discovered() {
    let root = unique_dir("roots");
    let cwd = root.join("proj");
    let agent = root.join("agent");

    write_artifact(&cwd.join(".cyrup").join("extensions"), "proj-tool.wasm");
    write_artifact(&agent.join("extensions"), "global-tool.wasm");
    // a configured path naming the artifact FILE itself (`cyrup --extension ./cfg-tool.wasm`).
    let cfg_artifact = write_artifact(&root.join("cfg"), "cfg-tool.wasm");

    let roots = DiscoveryRoots {
        project_cwd: Some(cwd),
        agent_dir: Some(agent),
        configured: vec![cfg_artifact.clone()],
    };
    let found = discover(&roots);

    let proj = find(&found, "proj-tool").expect("bare artifact in the PROJECT root is discovered");
    assert_eq!(proj.origin, ExtOrigin::Project);
    assert!(proj.wasm.as_deref().map(|w| w.ends_with("proj-tool.wasm")).unwrap_or(false));

    let global = find(&found, "global-tool").expect("bare artifact in the GLOBAL root is discovered");
    assert_eq!(global.origin, ExtOrigin::Global);
    assert!(global.wasm.as_deref().map(|w| w.ends_with("global-tool.wasm")).unwrap_or(false));

    let cfg = find(&found, "cfg-tool").expect("a configured path naming an artifact file is discovered");
    assert_eq!(cfg.origin, ExtOrigin::Configured);
    assert_eq!(cfg.wasm.as_deref(), Some(cfg_artifact.as_path()));

    let _ = std::fs::remove_dir_all(&root);
}

/// Several bare artifacts share one root, so the dedup key must be the ARTIFACT, not the containing
/// directory — otherwise the second and later artifacts collapse into the first. Directory-shaped
/// extensions in the same root keep working alongside them.
#[test]
fn multiple_bare_artifacts_and_dirs_coexist_in_one_root() {
    let root = unique_dir("mixed");
    let agent = root.join("agent");
    let ext_root = agent.join("extensions");

    write_artifact(&ext_root, "alpha.wasm");
    write_artifact(&ext_root, "beta.wasm");
    // a conventional directory-shaped extension beside them.
    let dir = ext_root.join("gamma");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("extension.json"),
        format!(r#"{{ "id": "gamma", "version": "1.0.0", "world": "{}" }}"#, cyrup_ext::HOST_WORLD),
    )
    .unwrap();
    std::fs::write(dir.join("gamma.wasm"), ARTIFACT).unwrap();
    // a non-artifact file in the root must still be ignored.
    std::fs::write(ext_root.join("README.md"), b"not an extension").unwrap();

    let roots = DiscoveryRoots {
        project_cwd: None,
        agent_dir: Some(agent),
        configured: vec![],
    };
    let found = discover(&roots);
    let ids: Vec<String> = found.iter().map(|d| d.manifest.id.clone()).collect();

    assert!(ids.contains(&"alpha".to_string()), "got {ids:?}");
    assert!(ids.contains(&"beta".to_string()), "second bare artifact not collapsed: {ids:?}");
    assert!(ids.contains(&"gamma".to_string()), "directory-shaped extension still found: {ids:?}");
    assert_eq!(found.len(), 3, "README.md is not an extension: {ids:?}");

    // Each bare artifact points at its OWN file (not `first_wasm`'s alphabetical winner).
    assert!(find(&found, "beta").unwrap().wasm.as_deref().unwrap().ends_with("beta.wasm"));

    let _ = std::fs::remove_dir_all(&root);
}

/// The discovery fix must reach the REAL load path, not just `discover()`: `discover_and_load` is
/// what `cyrup-session-svc`'s builder calls (`builder.rs:922`, and the pre-trust probe at `:1639`).
/// An untrusted project makes the trust gate — not wasm parsing — the first thing the bare artifact
/// hits, so this asserts the artifact was actually handed to the loader without needing a real
/// component. Before the fix the artifact was invisible and `errors` came back empty.
#[cfg(feature = "wasm-host")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bare_artifact_reaches_discover_and_load() {
    use cyrup_ext::{DenyServices, ExtMode, ExtensionHost, HostConfig};
    use std::sync::Arc;

    let root = unique_dir("load");
    let cwd = root.join("proj");
    write_artifact(&cwd.join(".cyrup").join("extensions"), "mytool.wasm");

    let roots = DiscoveryRoots {
        project_cwd: Some(cwd.clone()),
        agent_dir: None,
        configured: vec![],
    };
    let cfg = HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: cwd.clone() };
    let host = ExtensionHost::with_wasm(cfg).expect("host with wasm");
    let services: Arc<dyn cyrup_ext::host::HostServices> = Arc::new(DenyServices);

    let res = host.discover_and_load(&roots, false, services).await;
    assert!(res.loaded.is_empty(), "untrusted project loads nothing: {:?}", res.loaded);
    assert_eq!(
        res.errors.len(),
        1,
        "the bare artifact reached the loader and was gated, errors={:?}",
        res.errors
    );
    assert!(
        res.errors[0].error.to_lowercase().contains("untrust"),
        "gated by project trust, got: {}",
        res.errors[0].error
    );

    let _ = std::fs::remove_dir_all(&root);
}
