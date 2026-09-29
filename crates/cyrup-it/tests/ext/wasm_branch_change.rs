//! EXT-064 LIVE-COMPONENT PROOF — a real `wasm32-wasip2` guest is pushed a branch change and
//! re-renders its footer from it, and `ui.set-footer` carries pi's `Component | undefined`
//! distinction across the WIT boundary.
//!
//! This is the half a native stand-in cannot prove. `cyrup-ext`'s `tests::branch_change` drives a
//! NATIVE extension through `ExtensionHost::branch_change`, and `cyrup-tui`'s
//! `extension_branch_change` drives the run-loop arm into that same host call — but neither crosses
//! the `events.on-branch-change` EXPORT or the re-signed `ui.set-footer` IMPORT, which are what the
//! `cyrup:ext@0.10 -> @0.11` bump is for. A guest built against the old world would fail to LINK
//! here, which is exactly what the bump exists to turn into `ExtError::WorldVersion` rather than an
//! opaque wasmtime error.
//!
//! **Upstream.** `ReadonlyFooterDataProvider.onBranchChange(callback: () => void): () => void`
//! ("Subscribe to git branch changes. Returns unsubscribe function.",
//! `core/footer-data-provider.ts:139-143` @v0.87.1), fired from `notifyBranchChange` (`:197-199`);
//! and `setFooter(factory: (…) => Component | undefined)`
//! (`core/extensions/types.ts:183-193` @v0.87.1), where `undefined` restores the built-in footer
//! (`modes/interactive/interactive-mode.ts:2442-2446`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::fixture;

use cyrup_ext::DenyServices;
use std::sync::Arc;

/// THE proof: the host pushes a branch change, the GUEST's callback runs across the WIT boundary,
/// and what it does with it — `ui.set-footer` carrying the new branch — comes back on the host's
/// recorded chrome.
///
/// **RED without the change:** `events.on-branch-change` did not exist in the world, so there was
/// no export to call and no `ui.subscribe-branch-change` for the guest's `init` to declare — this
/// file does not compile, and the fixture component does not link, against the pre-change tree.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_guest_is_pushed_the_new_branch_and_re_renders_its_footer() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    // `DenyServices` is enough: the assertions read the GUEST-side recorder
    // (`LiveExtension::guest()`), which is where `notify` and `set-footer` land for a guest whose
    // host backend grants nothing — exactly as `wasm_bus_flag` reads it.
    let ext = host
        .load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init the live wasm extension");

    // The guest's `init` declared the callback, so the host-side subscription exists — this is the
    // `ui.subscribe-branch-change` import having actually been called across the boundary, which
    // `load_wasm` returning `Ok` on its own does not show.
    assert!(
        host.has_branch_change_subscribers(),
        "the guest declared `on_branch_change`, so `ui.subscribe-branch-change` must have reached \
         the registry"
    );

    host.branch_change(Some("feature/x")).await;

    let notes = ext.guest().notifications();
    assert!(
        notes
            .iter()
            .any(|n| n == "demo: branch changed to feature/x"),
        "the guest's callback ran across the `on-branch-change` export with the new branch; \
         notes: {notes:?}"
    );

    // …and it RE-RENDERED: the whole reason the push exists is that a cyrup guest hands over
    // rendered text and cannot re-read `getGitBranch()` on a repaint the way pi's footer COMPONENT
    // does. `Some(..)` is pi's factory; the outer `Some` is "set-footer was called at all".
    assert_eq!(
        ext.guest().chrome().footer,
        Some(Some("branch: feature/x".to_string())),
        "the guest set a footer carrying the new branch"
    );

    // The tri-state travels: `none` is pi's `getGitBranch()` returning `null` — not in a repo
    // (`core/footer-data-provider.ts:126-132` @v0.87.1) — and it must arrive as `none`, not as an
    // empty string.
    host.branch_change(None).await;
    let notes = ext.guest().notifications();
    assert!(
        notes
            .iter()
            .any(|n| n == "demo: branch changed to (no repo)"),
        "`none` reached the guest unflattened; notes: {notes:?}"
    );
}
