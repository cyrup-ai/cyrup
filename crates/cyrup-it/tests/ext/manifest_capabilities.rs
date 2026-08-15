//! EXT-054 / EXT-055 — the declared per-extension sandbox actually restricts the guest.
//!
//! This is the regression fixture the item's **Verify** block asks for, driven the way the defect
//! was reproduced live on 2026-08-13 (`docs/gap-analysis/REPRO-LOG.md`, "EXT-054 — CONFIRMED"): the
//! real `cyrup-ext-sdk` component, laid out in a real extension directory with a real
//! `extension.json`, loaded through the REAL production path
//! (`discover_and_load` -> `load_discovered` -> `load_wasm_with_caps`) — not through `load_wasm`,
//! which is the manifest-less host-internal entry and is deliberately NOT capped by a manifest it
//! has never been given.
//!
//! Each capability is asserted at the `HostServices` boundary, not just in the guest's own error
//! text: a denied guest must produce **zero** `exec_calls` / `http_requests` on the recording
//! backend. That is the difference between "the guest was told no" and "the host never let the call
//! through" — and it is the assertion that would have caught the original defect, in which a fully
//! capable `LiveHostServices` happily ran `echo hi` for a guest declaring `"exec": false`.
//!
//! Before the fix every one of the `*_denied_*` tests below FAILS: `load_discovered` called
//! `self.load_wasm(id, &bytes, services)`, a signature with no manifest parameter, so
//! `disc.manifest.capabilities` was parsed and dropped on the floor.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::CancelToken;
use cyrup_ext::loader::DiscoveryRoots;
use cyrup_ext::{
    CannedResponses, ExtMode, ExtensionHost, HostConfig, RecordingServices, DENIED_EXEC, DENIED_NET,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The `wasm32-wasip2` guest component. Built ONCE for the whole suite by
/// `crates/cyrup-it/build.rs` and handed over as `CYRUP_IT_COMPONENT`; this replaces the nested
/// `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` that used to live here.
/// `CYRUP_EXT_FIXTURE_COMPONENT` still overrides it — now read in one place instead of thirteen.
fn fixture_component() -> PathBuf {
    crate::support::bins::component()
}

fn temp_project(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("cyrup-ext-caps-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Lay out `<cwd>/.cyrup/extensions/demo/{extension.json, demo.wasm}` with `caps_json` as the
/// manifest's `capabilities` block, and return the project cwd.
fn project_with_caps(name: &str, caps_json: &str) -> PathBuf {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component");
    let cwd = temp_project(name);
    let ext_dir = cwd.join(".cyrup").join("extensions").join("demo");
    std::fs::create_dir_all(&ext_dir).unwrap();
    // EXT-028: interpolate `HOST_WORLD` so a world bump does not silently stop this fixture from
    // reaching instantiation (`check_world` would refuse it first).
    std::fs::write(
        ext_dir.join("extension.json"),
        format!(
            r#"{{ "id": "demo", "version": "1.0.0", "world": "{}", "capabilities": {caps_json} }}"#,
            cyrup_ext::HOST_WORLD
        ),
    )
    .unwrap();
    std::fs::write(ext_dir.join("demo.wasm"), &bytes).unwrap();
    cwd
}

/// The same layout with **no `capabilities` key at all** — the shape [`project_with_caps`] cannot
/// produce, because it always interpolates the key. `ExtensionManifest::capabilities` is
/// `#[serde(default)]` over a `Capabilities` whose every field is `#[serde(default)]`
/// (`cyrup-ext/src/manifest.rs:23-24,43-50`), so this must deserialize to `Capabilities::none()`.
fn project_without_caps_key(name: &str) -> PathBuf {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component");
    let cwd = temp_project(name);
    let ext_dir = cwd.join(".cyrup").join("extensions").join("demo");
    std::fs::create_dir_all(&ext_dir).unwrap();
    std::fs::write(
        ext_dir.join("extension.json"),
        format!(
            r#"{{ "id": "demo", "version": "1.0.0", "world": "{}" }}"#,
            cyrup_ext::HOST_WORLD
        ),
    )
    .unwrap();
    std::fs::write(ext_dir.join("demo.wasm"), &bytes).unwrap();
    cwd
}

/// Load the fixture from `cwd` through the production discovery path, as a TRUSTED project.
async fn load(cwd: &Path) -> (ExtensionHost, Arc<RecordingServices>) {
    let roots = DiscoveryRoots {
        project_cwd: Some(cwd.to_path_buf()),
        agent_dir: None,
        configured: vec![],
    };
    let cfg = HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: cwd.to_path_buf() };
    let host = ExtensionHost::with_wasm(cfg).expect("host with wasm");
    // A fully capable, non-deny backend: if anything still gets through, it is the HOST that let it.
    let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
    let res = host.discover_and_load(&roots, true, rec.clone()).await;
    assert_eq!(res.loaded.len(), 1, "fixture loaded, errors={:?}", res.errors);
    (host, rec)
}

// ---------------------------------------------------------------------------------------------
// exec
// ---------------------------------------------------------------------------------------------

/// The reproduction, inverted. `{"exec": false}` and the host never reaches its exec backend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_is_refused_when_the_manifest_denies_it() {
    let cwd = project_with_caps("exec-deny", r#"{ "fs": [], "exec": false, "net": false, "ui": false }"#);
    let (host, rec) = load(&cwd).await;

    let out = host.run_command("execdemo", "", &CancelToken::new()).await.expect("command runs");
    let out = out.unwrap_or_default();
    assert!(out.contains("exec denied"), "guest saw a denial, got: {out}");
    assert!(out.contains(DENIED_EXEC), "denial names the manifest key, got: {out}");
    assert!(
        rec.exec_calls().is_empty(),
        "the host refused BEFORE the exec backend: {:?}",
        rec.exec_calls()
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

/// The other direction: the grant is a real grant, not a blanket refusal. Without this the "fix"
/// could be `deny everything`, which is not a sandbox — it is a broken loader.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_runs_when_the_manifest_grants_it() {
    let cwd = project_with_caps("exec-grant", r#"{ "exec": true, "ui": true }"#);
    let (host, rec) = load(&cwd).await;

    let out = host.run_command("execdemo", "", &CancelToken::new()).await.expect("command runs");
    let out = out.unwrap_or_default();
    assert!(!out.contains("denied"), "granted exec is not denied, got: {out}");
    assert_eq!(
        rec.exec_calls(),
        vec![("echo".to_string(), vec!["hi".to_string()])],
        "the exec reached the backend"
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

// ---------------------------------------------------------------------------------------------
// net
// ---------------------------------------------------------------------------------------------

/// The live repro opened a real TLS connection from a `{"net": false}` guest. Now the request never
/// reaches the http backend at all — asserted at the backend, so no network is touched either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn net_is_refused_when_the_manifest_denies_it() {
    let cwd = project_with_caps("net-deny", r#"{ "fs": [], "exec": false, "net": false, "ui": true }"#);
    let (host, rec) = load(&cwd).await;

    let out = host
        .run_command("httpdemo", "https://example.invalid/", &CancelToken::new())
        .await
        .expect("command runs");
    let out = out.unwrap_or_default();
    assert!(out.contains("http denied"), "guest saw a denial, got: {out}");
    assert!(out.contains(DENIED_NET), "denial names the manifest key, got: {out}");
    assert!(
        rec.http_requests().is_empty(),
        "the host refused BEFORE the http backend: {:?}",
        rec.http_requests()
    );

    // The streaming half is the same capability and must be gated identically — the original defect
    // was per-INTERFACE, not per-function, and a fix that only covered `request` would leave
    // `request-stream` as an open second door to the same network.
    let out = host
        .run_command("httpstreamdemo", "https://example.invalid/", &CancelToken::new())
        .await
        .expect("command runs");
    assert!(
        rec.http_requests().is_empty(),
        "request-stream refused too: {:?} (guest said: {out:?})",
        rec.http_requests()
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn net_runs_when_the_manifest_grants_it() {
    let cwd = project_with_caps("net-grant", r#"{ "net": true, "ui": true }"#);
    let (host, rec) = load(&cwd).await;

    // `RecordingServices` answers with a canned response, so this asserts the GATE, not the network.
    let out = host
        .run_command("httpdemo", "https://example.invalid/", &CancelToken::new())
        .await
        .expect("command runs");
    let out = out.unwrap_or_default();
    assert!(!out.contains("denied"), "granted net is not denied, got: {out}");
    assert_eq!(rec.http_requests().len(), 1, "the request reached the backend");

    let _ = std::fs::remove_dir_all(&cwd);
}

// ---------------------------------------------------------------------------------------------
// zero declarations — the DEFAULT, which is the shape a real authoring mistake produces
// ---------------------------------------------------------------------------------------------

/// A guest declaring **zero** capabilities is refused exec AND net.
///
/// Every `*_is_refused_when_the_manifest_denies_it` test above writes the bits out EXPLICITLY
/// (`"exec": false, "net": false`). That pins the explicit-deny path and leaves the default path —
/// an author who simply omits the key, which is what a hand-written `extension.json` actually looks
/// like — resting on `#[serde(default)]` and on the module doc's claim at
/// `cyrup-ext/src/manifest.rs:31-33` that "a manifest with no `capabilities` block … grant[s]
/// nothing at all". Only `fs` had that claim observed
/// ([`fs_is_refused_when_no_grant_is_declared`]); for `exec` and `net` it was read, never run. A
/// `#[serde(default)]` that someone later replaces with a custom `Deserialize`, or a field that
/// gains a non-`false` default, fails HERE and nowhere else.
///
/// Both zero shapes are covered: an empty `capabilities: {}` object and no `capabilities` key at
/// all. They reach the same `Capabilities::none()` by different serde routes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_declaring_zero_capabilities_is_refused_exec_and_net() {
    for (label, cwd) in [
        ("empty capabilities object", project_with_caps("caps-empty", "{}")),
        ("no capabilities key", project_without_caps_key("caps-absent")),
    ] {
        let (host, rec) = load(&cwd).await;

        // ---- exec ----
        let out = host
            .run_command("execdemo", "", &CancelToken::new())
            .await
            .expect("command runs")
            .unwrap_or_default();
        assert!(out.contains("exec denied"), "{label}: guest saw an exec denial, got: {out}");
        assert!(out.contains(DENIED_EXEC), "{label}: denial names the manifest key, got: {out}");
        assert!(
            rec.exec_calls().is_empty(),
            "{label}: the host refused BEFORE the exec backend: {:?}",
            rec.exec_calls()
        );

        // ---- net, both doors (`request` and `request-stream`) ----
        let out = host
            .run_command("httpdemo", "https://example.invalid/", &CancelToken::new())
            .await
            .expect("command runs")
            .unwrap_or_default();
        assert!(out.contains("http denied"), "{label}: guest saw a net denial, got: {out}");
        assert!(out.contains(DENIED_NET), "{label}: denial names the manifest key, got: {out}");
        assert!(
            rec.http_requests().is_empty(),
            "{label}: the host refused BEFORE the http backend: {:?}",
            rec.http_requests()
        );

        let out = host
            .run_command("httpstreamdemo", "https://example.invalid/", &CancelToken::new())
            .await
            .expect("command runs");
        assert!(
            rec.http_requests().is_empty(),
            "{label}: request-stream refused too: {:?} (guest said: {out:?})",
            rec.http_requests()
        );

        let _ = std::fs::remove_dir_all(&cwd);
    }
}

/// The companion that keeps the test above from being vacuous. `load()` installs a FULLY CAPABLE
/// `RecordingServices`, so "zero exec calls" would also be the reading if the demo commands were
/// misnamed, the guest never instantiated, or the backend were a deny-stub. Granting the very same
/// bits on the very same fixture must produce a NON-empty backend record — which is what makes the
/// emptiness above an enforced refusal rather than an absence of activity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_zero_capability_refusal_is_a_refusal_and_not_an_absence_of_activity() {
    let cwd = project_with_caps("caps-empty-control", r#"{ "exec": true, "net": true }"#);
    let (host, rec) = load(&cwd).await;

    let out = host
        .run_command("execdemo", "", &CancelToken::new())
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(!out.contains("denied"), "control: granted exec is not denied, got: {out}");
    assert_eq!(
        rec.exec_calls(),
        vec![("echo".to_string(), vec!["hi".to_string()])],
        "control: the SAME `execdemo` command reaches the backend once granted"
    );

    let out = host
        .run_command("httpdemo", "https://example.invalid/", &CancelToken::new())
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(!out.contains("denied"), "control: granted net is not denied, got: {out}");
    assert_eq!(
        rec.http_requests().len(),
        1,
        "control: the SAME `httpdemo` command reaches the backend once granted"
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

// ---------------------------------------------------------------------------------------------
// ui
// ---------------------------------------------------------------------------------------------

/// `interface ui` is fire-and-forget in most of its members, so the guest cannot observe its own
/// refusal — which is exactly why the live repro could report "the `ui` bit is inert too" only by
/// watching the host. Watch the host: an all-false manifest produces no notification at the
/// backend, while the identical run with `"ui": true` produces one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_effects_are_dropped_when_the_manifest_denies_ui() {
    let denied = project_with_caps("ui-deny", r#"{ "fs": [], "exec": false, "net": false, "ui": false }"#);
    let (host, rec) = load(&denied).await;
    let _ = host.run_command("execdemo", "", &CancelToken::new()).await.expect("command runs");
    assert!(
        rec.notify_calls().is_empty(),
        "ui.notify never reached the backend: {:?}",
        rec.notify_calls()
    );
    drop(host);
    let _ = std::fs::remove_dir_all(&denied);

    let granted = project_with_caps("ui-grant", r#"{ "ui": true }"#);
    let (host, rec) = load(&granted).await;
    let _ = host.run_command("execdemo", "", &CancelToken::new()).await.expect("command runs");
    assert!(
        !rec.notify_calls().is_empty(),
        "the same run WITH the ui grant does notify — so the denial above is the grant, not a \
         missing notification"
    );
    let _ = std::fs::remove_dir_all(&granted);
}

// ---------------------------------------------------------------------------------------------
// fs (EXT-055)
// ---------------------------------------------------------------------------------------------

/// No `capabilities.fs` entry => `ext-fs` is refused, and the refusal names the key that would
/// grant it. This is the fail-CLOSED mirror of EXT-054 that EXT-055 filed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fs_is_refused_when_no_grant_is_declared() {
    let cwd = project_with_caps("fs-none", r#"{ "ui": true }"#);
    let (host, _rec) = load(&cwd).await;

    let out = host
        .run_command("fswrite", "note.txt hello", &CancelToken::new())
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(out.contains("denied"), "write refused, got: {out}");
    assert!(out.contains("capabilities.fs"), "refusal names the manifest key, got: {out}");
    assert!(!cwd.join("note.txt").exists(), "nothing was written");

    let _ = std::fs::remove_dir_all(&cwd);
}

/// A `write:` grant is honoured, scoped to the subtree it names, and a `read:` grant does NOT imply
/// write — the two modes the manifest syntax has always had and nothing ever read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fs_grants_are_scoped_by_mode_and_by_subtree() {
    let cwd = project_with_caps("fs-scoped", r#"{ "fs": ["read:.", "write:.cyrup/todo"], "ui": true }"#);
    std::fs::create_dir_all(cwd.join(".cyrup").join("todo")).unwrap();
    std::fs::write(cwd.join("readable.txt"), b"visible").unwrap();
    let (host, _rec) = load(&cwd).await;
    let cancel = CancelToken::new();

    // read: covers the whole project.
    let out = host
        .run_command("fsread", "readable.txt", &cancel)
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(out.contains("visible"), "the read: grant reads, got: {out}");

    // write: covers only `.cyrup/todo`.
    let out = host
        .run_command("fswrite", ".cyrup/todo/item.md hello", &cancel)
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(!out.contains("denied"), "the write: grant writes, got: {out}");
    assert_eq!(
        std::fs::read_to_string(cwd.join(".cyrup").join("todo").join("item.md")).unwrap(),
        "hello"
    );

    // ...and nowhere else, even though `read:.` covers the same path.
    let out = host
        .run_command("fswrite", "escaped.txt hello", &cancel)
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(out.contains("denied"), "a read:-only path is not writable, got: {out}");
    assert!(!cwd.join("escaped.txt").exists(), "nothing was written outside the write: grant");

    // A `..` escape is refused by the resolver regardless of grants.
    let out = host
        .run_command("fswrite", "../escape.txt hello", &cancel)
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(out.contains("escapes the granted capability root"), "got: {out}");

    let _ = std::fs::remove_dir_all(&cwd);
}

/// A malformed grant fails the LOAD rather than being silently dropped: a typo in the sandbox
/// declaration must not quietly widen or narrow it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_fs_grant_fails_the_load() {
    let cwd = project_with_caps("fs-bad", r#"{ "fs": ["read:../../etc"] }"#);
    let roots = DiscoveryRoots {
        project_cwd: Some(cwd.clone()),
        agent_dir: None,
        configured: vec![],
    };
    let cfg = HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: cwd.clone() };
    let host = ExtensionHost::with_wasm(cfg).expect("host with wasm");
    let rec = Arc::new(RecordingServices::new(CannedResponses::default()));

    let res = host.discover_and_load(&roots, true, rec).await;
    assert!(res.loaded.is_empty(), "the extension did not load: {:?}", res.loaded);
    assert_eq!(res.errors.len(), 1, "one recorded error: {:?}", res.errors);
    assert!(
        res.errors[0].error.contains("invalid capability declaration"),
        "typed capability error, got: {}",
        res.errors[0].error
    );
    assert!(res.errors[0].fatal, "a malformed sandbox declaration is a fatal load fault");

    let _ = std::fs::remove_dir_all(&cwd);
}

// ---------------------------------------------------------------------------------------------
// the manifest-less entry point
// ---------------------------------------------------------------------------------------------

/// `load_wasm` is the host's OWN entry point — no `extension.json` exists to read — so it keeps the
/// pre-EXT-054 grant (`Capabilities::host_granted`). Pinned so a later change cannot narrow it by
/// accident and silently break every `AgentSession::load_wasm_extension` caller, nor widen it into
/// an `fs` grant it never had.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_manifest_less_load_keeps_the_host_grant() {
    let caps = cyrup_ext::Capabilities::host_granted();
    assert!(caps.exec && caps.net && caps.ui, "interactive capabilities stay on");
    assert!(caps.fs.is_empty(), "but `ext-fs` still has no root without a declared grant");
    assert_eq!(cyrup_ext::Capabilities::none(), cyrup_ext::Capabilities::default());

    let bytes = std::fs::read(fixture_component()).expect("read fixture component");
    let cwd = temp_project("hostgrant");
    let cfg = HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: cwd.clone() };
    let host = ExtensionHost::with_wasm(cfg).expect("host with wasm");
    let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
    host.load_wasm("demo".into(), &bytes, rec.clone()).await.expect("load + init");

    let out = host
        .run_command("execdemo", "", &CancelToken::new())
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(!out.contains("denied"), "the host-internal load still grants exec, got: {out}");
    assert_eq!(rec.exec_calls().len(), 1);

    let _ = std::fs::remove_dir_all(&cwd);
}

/// The companion an embedder that wants LESS than the full host grant must reach for:
/// `load_wasm_with_caps` takes the grant as an argument, so a manifest-less component supplied by
/// the embedder can still be capped.
///
/// This is the API-shaped half of the `AgentSession::load_wasm_extension` escape hatch. That
/// wrapper routes to `load_wasm` and therefore hands an embedder-supplied guest
/// `Capabilities::host_granted()` — which is what PARITY requires on its own (Pi's embedder seam,
/// `loadExtensionFromFactory` at `packages/coding-agent/src/core/extensions/loader.ts:485-498`
/// @v0.83.0, and `DefaultResourceLoader`'s `extensionFactories` in `examples/sdk/06-extensions.ts`,
/// build the extension from the caller's own code and hand it the complete `ExtensionAPI`; Pi has no
/// capability model to narrow). The gap is not that `load_wasm` grants too much, it is that no
/// capped companion is reachable from the session layer. This pins that the ext-layer half of that
/// companion works — the same bytes, the same fully capable `RecordingServices`, restricted purely
/// by the grant passed in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_explicit_grant_entry_point_caps_a_manifest_less_component() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component");
    let cwd = temp_project("explicitcaps");
    let cfg = HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: cwd.clone() };
    let host = ExtensionHost::with_wasm(cfg).expect("host with wasm");
    // Fully capable backend: anything that gets through got through the HOST, not the stub.
    let rec = Arc::new(RecordingServices::new(CannedResponses::default()));

    // `ui` on so the guest can still report; `exec` off is the restriction under test.
    let caps = cyrup_ext::Capabilities { fs: Vec::new(), exec: false, net: false, ui: true };
    host.load_wasm_with_caps("demo".into(), &bytes, rec.clone(), &caps)
        .await
        .expect("load + init");

    let out = host
        .run_command("execdemo", "", &CancelToken::new())
        .await
        .expect("command runs")
        .unwrap_or_default();
    assert!(out.contains("exec denied"), "the explicit grant restricts the guest, got: {out}");
    assert!(out.contains(DENIED_EXEC), "denial names the capability key, got: {out}");
    assert!(
        rec.exec_calls().is_empty(),
        "the host refused BEFORE the exec backend: {:?}",
        rec.exec_calls()
    );

    let _ = std::fs::remove_dir_all(&cwd);
}
