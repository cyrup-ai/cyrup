//! A malformed `extension.json` must not vanish.
//!
//! Before this fixture, `loader.rs::push_dir` swallowed `Err(_)` from `ExtensionManifest::load`
//! and fell straight through to the manifest-less "bare `.wasm`" rule. The extension still loaded,
//! but under a DIFFERENT id (the artifact stem, not the declared one) and holding
//! `Capabilities::none()`, with nothing written to any channel — so an author who put a trailing
//! comma in the manifest got a differently-named, powerless extension and no message at all.
//!
//! # What Pi does, exactly (this is the whole parity argument)
//!
//! Pi's manifest reader swallows a malformed `package.json` outright —
//! `packages/coding-agent/src/core/extensions/loader.ts:568-579` @v0.83.0 is
//! `try { … JSON.parse(content) … } catch { return null }` — and `resolveExtensionEntries`
//! (`loader.ts:594-624`) then falls through to the `index.ts` / `index.js` convention, or returns
//! `null` and `discoverExtensionsInDir` (`loader.ts:636-668`) drops the subdirectory. Pi records
//! NOTHING in `LoadExtensionsResult.errors` for any of it and startup continues.
//!
//! So the load OUTCOME ported here is Pi's — fall back, keep going, never abort startup, hence
//! `LoadError::fatal == false`. The only thing added is the message, because Pi can afford the
//! silence and cyrup cannot: Pi's `pi.extensions` manifest is a pointer list, so its fallback yields
//! the same extension at the same path-derived identity with the same (total) privileges, whereas
//! cyrup's `extension.json` also carries the id and the capability grant.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::loader::{discover, discover_with_diagnostics, DiscoveryRoots};
use std::path::{Path, PathBuf};

/// A stand-in artifact. Discovery never inspects the bytes.
const ARTIFACT: &[u8] = b"\0asm\x0d\x00\x01\x00";

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir()
        .join(format!("cyrup-ext-badmanifest-{tag}-{nanos}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `<cwd>/.cyrup/extensions/<name>/` holding `manifest` (when `Some`) and an artifact (when
/// `artifact` is `Some`). Returns the project cwd.
fn project_with(name: &str, manifest: Option<&str>, artifact: Option<&str>) -> PathBuf {
    let cwd = unique_dir(name);
    let ext_dir = cwd.join(".cyrup").join("extensions").join(name);
    std::fs::create_dir_all(&ext_dir).unwrap();
    if let Some(m) = manifest {
        std::fs::write(ext_dir.join("extension.json"), m).unwrap();
    }
    if let Some(a) = artifact {
        std::fs::write(ext_dir.join(a), ARTIFACT).unwrap();
    }
    cwd
}

fn project_roots(cwd: &Path) -> DiscoveryRoots {
    DiscoveryRoots {
        project_cwd: Some(cwd.to_path_buf()),
        agent_dir: None,
        configured: vec![],
    }
}

/// The core case. `{ "id": "declared",` — a truncated manifest — beside `payload.wasm`.
///
/// Pi's control flow is kept verbatim: the directory still yields exactly one extension, taken from
/// the fallback rule. What changes is that the fallback is no longer silent — one NON-FATAL
/// diagnostic names the manifest, carries the parse error, and states both consequences the author
/// would otherwise have to infer (the id it actually loaded under, and the empty grant).
#[test]
fn a_malformed_manifest_beside_an_artifact_is_reported_and_falls_back() {
    let cwd = project_with("broken", Some(r#"{ "id": "declared","#), Some("payload.wasm"));
    let (found, diags) = discover_with_diagnostics(&project_roots(&cwd));

    // Pi's outcome, unchanged: the fallback extension exists and is loadable.
    assert_eq!(found.len(), 1, "the fallback still discovers the artifact: {found:?}");
    assert_eq!(found[0].manifest.id, "payload", "id came from the artifact stem, not the manifest");
    assert_eq!(
        found[0].manifest.capabilities,
        cyrup_ext::Capabilities::none(),
        "a manifest nobody could read grants nothing"
    );
    assert_eq!(discover(&project_roots(&cwd)).len(), 1, "`discover` behaves identically");

    // The part that used to be missing entirely.
    assert_eq!(diags.len(), 1, "exactly one diagnostic: {diags:?}");
    assert!(
        !diags[0].fatal,
        "non-fatal: Pi falls back and finishes startup (loader.ts:568-579), so cyrup must not exit 1"
    );
    assert!(
        diags[0].path.ends_with("broken"),
        "the diagnostic points at the offending directory, got {}",
        diags[0].path.display()
    );
    let msg = &diags[0].error;
    assert!(msg.contains("extension.json"), "names the file, got: {msg}");
    assert!(msg.contains("payload"), "names the id it actually loaded under, got: {msg}");
    assert!(
        msg.contains("NO declared capabilities"),
        "states the capability consequence, got: {msg}"
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

/// The other half of Pi's fall-through: no `index.ts` to fall back to means the subdirectory
/// contributes nothing (`loader.ts:594-624` returns `null`). cyrup's analog is "no `.wasm`" — the
/// directory is skipped, and skipping it is exactly the outcome an author must be told about,
/// because there is now no extension at all to notice the absence of.
#[test]
fn a_malformed_manifest_with_no_artifact_is_reported_and_skipped() {
    let cwd = project_with("nowasm", Some("this is not json"), None);
    let (found, diags) = discover_with_diagnostics(&project_roots(&cwd));

    assert!(found.is_empty(), "nothing to fall back to: {found:?}");
    assert_eq!(diags.len(), 1, "the skip is still reported: {diags:?}");
    assert!(!diags[0].fatal, "Pi does not abort startup for this either");
    assert!(
        diags[0].error.contains("was skipped"),
        "says the directory was skipped, got: {}",
        diags[0].error
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

/// The guard against over-reporting: a directory with NO `extension.json` is the plain
/// manifest-less convention — Pi's directory with no `package.json` and an `index.ts`
/// (`loader.ts:613-621`) — and must stay silent. Without this assertion the "fix" could be a
/// diagnostic on every bare-artifact extension, which would turn a supported layout into noise.
#[test]
fn a_directory_with_no_manifest_at_all_produces_no_diagnostic() {
    let cwd = project_with("plain", None, Some("plain.wasm"));
    let (found, diags) = discover_with_diagnostics(&project_roots(&cwd));

    assert_eq!(found.len(), 1, "the manifest-less rule still works: {found:?}");
    assert_eq!(found[0].manifest.id, "plain");
    assert!(diags.is_empty(), "an absent manifest is not an error: {diags:?}");

    let _ = std::fs::remove_dir_all(&cwd);
}

/// A well-formed manifest is untouched — no diagnostic, and the DECLARED id wins over the artifact
/// stem. This is the control that proves the two preceding tests observe the malformed path
/// specifically and not merely "any directory with a manifest".
#[test]
fn a_valid_manifest_produces_no_diagnostic_and_keeps_its_declared_id() {
    let manifest = format!(
        r#"{{ "id": "declared", "version": "1.0.0", "world": "{}" }}"#,
        cyrup_ext::HOST_WORLD
    );
    let cwd = project_with("valid", Some(&manifest), Some("payload.wasm"));
    let (found, diags) = discover_with_diagnostics(&project_roots(&cwd));

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].manifest.id, "declared", "the declared id wins when the manifest parses");
    assert!(diags.is_empty(), "a valid manifest is silent: {diags:?}");

    let _ = std::fs::remove_dir_all(&cwd);
}

/// The diagnostic has to reach the channel the session actually reads —
/// `LoadExtensionsResult.errors`, which `cyrup-session-svc`'s builder folds into the startup
/// `[Extension issues]` panel — not just `discover_with_diagnostics`'s return value.
///
/// The project is TRUSTED and the artifact is deliberate garbage, so the load fault that follows is
/// `fatal: true` and sits right beside the `fatal: false` manifest diagnostic. That contrast is the
/// point: this asserts the malformed manifest does NOT become an exit-1 startup abort, which is the
/// behaviour Pi's `catch { return null }` fall-through pins.
#[cfg(feature = "wasm-host")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_diagnostic_reaches_discover_and_load_without_becoming_fatal() {
    use cyrup_ext::{DenyServices, ExtMode, ExtensionHost, HostConfig};
    use std::sync::Arc;

    let cwd = project_with("reaches", Some(r#"{ "id": "declared","#), Some("payload.wasm"));
    let cfg = HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: cwd.clone() };
    let host = ExtensionHost::with_wasm(cfg).expect("host with wasm");
    let services: Arc<dyn cyrup_ext::host::HostServices> = Arc::new(DenyServices);

    let res = host.discover_and_load(&project_roots(&cwd), true, services).await;

    assert!(res.loaded.is_empty(), "the garbage artifact does not instantiate: {:?}", res.loaded);
    let manifest_diag = res
        .errors
        .iter()
        .find(|e| e.error.contains("extension.json"))
        .expect("the manifest diagnostic reached LoadExtensionsResult.errors");
    assert!(!manifest_diag.fatal, "a malformed manifest is not an exit-1 startup abort");
    assert!(
        res.errors.iter().any(|e| e.fatal),
        "and the genuine load fault beside it still IS fatal: {:?}",
        res.errors
    );

    let _ = std::fs::remove_dir_all(&cwd);
}
