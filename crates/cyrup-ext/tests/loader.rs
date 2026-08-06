//! Extension discovery + trust-split tests (arch-08 §6.2; Pi `discoverAndLoadExtensions`). Pure
//! filesystem scan — no wasm runtime — exercising the three-root scan, dedup, manifest parsing,
//! origin attribution, and the pre/post-trust eligibility split (R-08-002).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_ext::loader::{discover, DiscoveryRoots, ExtOrigin};
use std::path::{Path, PathBuf};

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("cyrup-ext-loader-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an extension dir `<base>/<name>/` with an `extension.json` (+ optional prebuilt artifact).
fn write_ext(base: &Path, name: &str, with_wasm: bool) -> PathBuf {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    // EXT-028: interpolate `HOST_WORLD` rather than a literal — a fixture pinned to a stale world
    // string silently stops exercising the load path the moment the world is bumped.
    let world = cyrup_ext::HOST_WORLD;
    let manifest =
        format!(r#"{{ "id": "{name}", "version": "1.0.0", "world": "{world}" }}"#);
    std::fs::write(dir.join("extension.json"), manifest).unwrap();
    if with_wasm {
        // a stand-in component artifact (discovery does not validate bytes).
        std::fs::write(dir.join("component.wasm"), b"\0asm\x0d\x00\x01\x00").unwrap();
    }
    dir
}

#[test]
fn discovers_three_roots_with_origins_and_dedup() {
    let root = unique_dir("roots");
    let cwd = root.join("proj");
    let agent = root.join("agent");
    let configured = root.join("cfg");

    let proj_ext = cwd.join(".cyrup").join("extensions");
    std::fs::create_dir_all(&proj_ext).unwrap();
    write_ext(&proj_ext, "proj-ext", true);

    let global_ext = agent.join("extensions");
    std::fs::create_dir_all(&global_ext).unwrap();
    write_ext(&global_ext, "global-ext", true);

    // a configured path pointing directly at an extension dir.
    std::fs::create_dir_all(&configured).unwrap();
    let cfg_ext = write_ext(&configured, "cfg-ext", true);

    let roots = DiscoveryRoots {
        project_cwd: Some(cwd.clone()),
        agent_dir: Some(agent.clone()),
        // include the SAME global dir again as a configured root: dedup must drop the repeat.
        configured: vec![cfg_ext, global_ext.join("global-ext")],
    };

    let found = discover(&roots);
    let ids: Vec<String> = found.iter().map(|d| d.manifest.id.clone()).collect();
    assert!(ids.contains(&"proj-ext".to_string()));
    assert!(ids.contains(&"global-ext".to_string()));
    assert!(ids.contains(&"cfg-ext".to_string()));
    // global-ext appears once despite being reachable via both the agent root and a configured path.
    assert_eq!(ids.iter().filter(|i| *i == "global-ext").count(), 1, "dedup by canonical path");

    let origin = |id: &str| found.iter().find(|d| d.manifest.id == id).map(|d| d.origin);
    assert_eq!(origin("proj-ext"), Some(ExtOrigin::Project));
    assert_eq!(origin("global-ext"), Some(ExtOrigin::Global));
    assert_eq!(origin("cfg-ext"), Some(ExtOrigin::Configured));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn trust_split_pre_and_post() {
    let root = unique_dir("trust");
    let cwd = root.join("proj");
    let agent = root.join("agent");
    let proj_ext = cwd.join(".cyrup").join("extensions");
    let global_ext = agent.join("extensions");
    std::fs::create_dir_all(&proj_ext).unwrap();
    std::fs::create_dir_all(&global_ext).unwrap();
    write_ext(&proj_ext, "proj-ext", true);
    write_ext(&global_ext, "global-ext", true);

    let roots = DiscoveryRoots {
        project_cwd: Some(cwd),
        agent_dir: Some(agent),
        configured: vec![],
    };
    let found = discover(&roots);
    let proj = found.iter().find(|d| d.manifest.id == "proj-ext").unwrap();
    let global = found.iter().find(|d| d.manifest.id == "global-ext").unwrap();

    // Untrusted project: the project-local ext is NOT eligible; the global one always is.
    assert!(!proj.is_trusted(false), "project-local is post-trust");
    assert!(global.is_trusted(false), "global is pre-trust");
    // Trusted project: both eligible.
    assert!(proj.is_trusted(true));
    assert!(global.is_trusted(true));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn bare_wasm_without_manifest_is_discovered() {
    let root = unique_dir("bare");
    let agent = root.join("agent");
    let global_ext = agent.join("extensions");
    let dir = global_ext.join("bare");
    std::fs::create_dir_all(&dir).unwrap();
    // a prebuilt component with NO extension.json: id is synthesized from the artifact stem.
    std::fs::write(dir.join("mytool.wasm"), b"\0asm\x0d\x00\x01\x00").unwrap();

    let roots = DiscoveryRoots {
        project_cwd: None,
        agent_dir: Some(agent),
        configured: vec![],
    };
    let found = discover(&roots);
    let bare = found.iter().find(|d| d.manifest.id == "mytool").expect("bare wasm discovered");
    assert!(bare.wasm.is_some());
    assert_eq!(bare.origin, ExtOrigin::Global);

    let _ = std::fs::remove_dir_all(&root);
}
