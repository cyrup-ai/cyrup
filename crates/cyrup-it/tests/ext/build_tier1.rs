//! Tier-1 build loop end-to-end (arch-08 §6.4; R-08-031). Drives the real `cargo build
//! --target wasm32-wasip2` invocation against an authored extension crate (the bundled
//! `cyrup-ext-sdk`), asserts it yields a valid `cyrup:ext` COMPONENT, that a second call is a
//! content-addressed cache HIT (no rebuild), and — since the `cyrup:ext@0.5` → `@0.6` bump — that
//! the artifact the loop produced actually INSTANTIATES against this host's world.
//!
//! MIGRATION NOTE — the one module in this target that keeps a nested `cargo build`. Everywhere
//! else in `tests/ext/` the nested wasip2 build was fixture scaffolding and was replaced by
//! `support::bins::component()`. Here it is the SUBJECT: `build_component_in` is production code
//! (`cyrup_ext::build`) and the assertions are about the build loop itself — that it emits a
//! component, that a second call hits the content-addressed cache instead of rebuilding, and that
//! what it emitted links. Handing it a prebuilt artifact would delete the test. It already writes
//! its cache to its own `TempDir`, so it does not contend for the workspace build lock.
//!
//! `env!("CARGO_MANIFEST_DIR")).join("../cyrup-ext-sdk")` below still resolves: `crates/cyrup-it`
//! and `crates/cyrup-ext` are siblings, so the relative path is unchanged by the move.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::build::{ArtifactCache, build_component_in, detect_toolchain};
// `Extension` is imported for its `subscriptions()` method, which is a TRAIT method on the loaded
// wasm extension (`host/live.rs:1752`) — the same import `wasm_component.rs` takes.
use cyrup_ext::{DenyServices, EventKind, Extension, ExtensionHost, HOST_WORLD, HostConfig};
use std::path::PathBuf;
use std::sync::Arc;

/// Multi-threaded on purpose: the body runs a BLOCKING `cargo build` (that is the subject under
/// test) and then drives the async wasm host on the same runtime. On a current-thread runtime the
/// blocking build would occupy the only worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tier1_cargo_build_emits_a_component_that_caches_and_instantiates() {
    // NOT a skip. `can_build()` is false only for `NoCargo` / `NoWasmTarget`
    // (`build/toolchain.rs:29-33`), and `crates/cyrup-it/build.rs:168-181` has ALREADY run
    // `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` for this binary and hard-failed if it
    // produced nothing — so inside this target both are present by construction. The `return` that
    // used to stand here made the whole module a green no-op the moment `rustup` was absent from
    // `$PATH`, which is the pass-that-proves-nothing this crate's build script was written to
    // eliminate ("NEVER silently skip", `build.rs:174-175`).
    let tc = detect_toolchain();
    assert!(
        tc.status.can_build(),
        "the wasm toolchain is a precondition of this whole test binary — build.rs already built a \
         {} component with it — but detect_toolchain reports {:?}: {}",
        tc.target,
        tc.status,
        tc.status.actionable().unwrap_or_default()
    );

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

    // ---------------------------------------------------------------------------------------
    // THE WORLD-BUMP PROOF. Everything above is a byte check: the preamble says "a component",
    // not "a component THIS host can link". Sweep 2 moved `HOST_WORLD` from `cyrup:ext@0.5` to
    // `@0.6`, and sweep 4 to `@0.7` (`types.tool-descriptor` gained `constrained-sampling`, which
    // re-signs the `registration.register-tool` import — PROV-011/EXT-024), on the strength of
    // host `bindgen!` accepting the new shapes and
    // `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` expanding `export_extension!` cleanly
    // — i.e. host and guest agreeing at the TYPE level, in two separate compilations that never
    // met. A world whose two copies have drifted, or an import re-signing the guest did not pick
    // up (`ui.set-widget` and `ui.theme-list` are both members of this bump,
    // `manifest.rs:169-176`), surfaces here and ONLY here, as an opaque wasmtime link error from
    // `load_wasm`'s instantiate step.
    //
    // `wasm_component.rs` also instantiates a guest, but a DIFFERENT artifact: the fixture
    // `crates/cyrup-it/build.rs` builds with a plain `cargo build -p cyrup-ext-sdk`. Nothing
    // instantiated the bytes the PRODUCTION Tier-1 loop returns until this assertion.
    // ---------------------------------------------------------------------------------------
    assert_eq!(HOST_WORLD, "cyrup:ext@0.7", "the world this artifact is being linked against");

    let host = ExtensionHost::with_wasm(HostConfig {
        mode: cyrup_ext::ExtMode::Tui,
        has_ui: true,
        cwd: crate_dir.clone(),
    })
    .expect("host with wasm runtime");

    let ext = host
        .load_wasm("tier1".into(), &bytes, Arc::new(DenyServices))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "the Tier-1 artifact does not link against {HOST_WORLD}: {e}\n\
                 A missing-import/missing-export error here means the two `world.wit` copies have \
                 drifted or the guest SDK was not rebuilt after the bump — the failure \
                 `check_world` exists to pre-empt (EXT-028)."
            )
        });

    // Instantiation alone would be satisfied by a guest whose `init` never ran. The demo
    // extension's `init` registers its hooks, so a non-empty subscription set proves the export
    // side of the world was reached too, not just the import side.
    let subs = ext.subscriptions();
    assert!(
        subs.contains(EventKind::ToolCall) && subs.contains(EventKind::AgentStart),
        "the guest's `init` ran across the boundary and declared its subscriptions: {subs:?}"
    );
}
