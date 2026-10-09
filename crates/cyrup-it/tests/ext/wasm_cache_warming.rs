//! `EXT-085` against a REAL `wasm32-wasip2` component — the guest half of pi's
//! `cache_warming_decision`.
//!
//! The in-workspace tests for this hook drive WAT components and the host's fold rule directly.
//! That proves the plumbing but not that a real guest built from the SDK reaches the export and
//! that its action survives the boundary, which is the claim this file exists to settle. The same
//! gap went unnoticed for the virtual-model guest tier until its gated suite was finally run.
//!
//! Upstream: `core/cache-warmer.ts:112-115` (the event), `:325-333` (where the refresh asks), and
//! `core/extensions/runner.ts:1121-1142` (`emitCacheWarmingDecision`, which assigns inside its loop
//! and never returns early — so the LAST readable action wins), read at v1.0.4.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cyrup_core::CancelToken;
use cyrup_ext::{CacheWarmingAction, DenyServices, ExtMode, ExtensionHost, HostConfig};
use std::sync::Arc;

use super::fixture;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// The demo guest's ceiling, mirrored rather than imported: `cyrup-ext-sdk`'s `example` module is
/// compiled to wasm, not linked here. If the two drift this test fails loudly, which is the point.
const DEMO_CEILING: f64 = 0.01;

async fn host_with_demo() -> ExtensionHost {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    host.load_wasm(
        "demo".into(),
        &bytes,
        Arc::new(DenyServices) as Arc<dyn cyrup_ext::HostServices>,
    )
    .await
    .expect("demo component loads");
    host
}

/// A refresh under the guest's ceiling: the guest answers "no opinion", so the host's own verdict
/// stands — including when the host said `warm`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_with_no_opinion_leaves_the_hosts_verdict_in_force() {
    let host = host_with_demo().await;
    let cancel = CancelToken::new();

    let action = host
        .aggregate_cache_warming_decision(
            DEMO_CEILING / 10.0,
            1.0,
            1.0,
            CacheWarmingAction::Warm,
            &cancel,
        )
        .await;
    assert_eq!(
        action,
        CacheWarmingAction::Warm,
        "a guest returning no opinion must not change the host's own decision"
    );

    // And the same in the other direction: no opinion cannot turn a host `stop` into a warm.
    let action = host
        .aggregate_cache_warming_decision(
            DEMO_CEILING / 10.0,
            0.0,
            1.0,
            CacheWarmingAction::Stop,
            &cancel,
        )
        .await;
    assert_eq!(action, CacheWarmingAction::Stop);
}

/// A refresh over the guest's ceiling: the guest's `stop` overrides a host `warm` across the
/// component boundary. This is the behaviour EXT-085 exists for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_guest_can_veto_a_warm_the_host_wanted() {
    let host = host_with_demo().await;
    let cancel = CancelToken::new();

    let action = host
        .aggregate_cache_warming_decision(
            DEMO_CEILING * 10.0,
            100.0,
            1.0,
            CacheWarmingAction::Warm,
            &cancel,
        )
        .await;
    assert_eq!(
        action,
        CacheWarmingAction::Stop,
        "the guest's cost ceiling must override the host's economics verdict"
    );
}

/// The payload reaches the guest intact: the guest's branch is chosen by `warm_cost`, so stepping
/// either side of the ceiling proves that field crossed the boundary with its value. A guest that
/// received 0.0 for everything would answer "no opinion" to both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_decision_payload_crosses_the_boundary_with_its_values() {
    let host = host_with_demo().await;
    let cancel = CancelToken::new();

    let under = host
        .aggregate_cache_warming_decision(
            DEMO_CEILING - 0.001,
            100.0,
            1.0,
            CacheWarmingAction::Warm,
            &cancel,
        )
        .await;
    let over = host
        .aggregate_cache_warming_decision(
            DEMO_CEILING + 0.001,
            100.0,
            1.0,
            CacheWarmingAction::Warm,
            &cancel,
        )
        .await;

    assert_eq!(under, CacheWarmingAction::Warm, "just under the ceiling");
    assert_eq!(over, CacheWarmingAction::Stop, "just over the ceiling");
}

/// Upstream contains a handler failure rather than letting it cancel warming
/// (`runner.ts:1134-1141`, pi's emit-error-and-continue). With no extension loaded at all there is
/// nobody to ask, which is the degenerate case of the same rule: the host's action stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_extension_loaded_the_hosts_action_stands() {
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let cancel = CancelToken::new();

    assert_eq!(
        host.aggregate_cache_warming_decision(1.0, 2.0, 1.0, CacheWarmingAction::Warm, &cancel)
            .await,
        CacheWarmingAction::Warm
    );
    assert_eq!(
        host.aggregate_cache_warming_decision(1.0, 2.0, 1.0, CacheWarmingAction::Stop, &cancel)
            .await,
        CacheWarmingAction::Stop
    );
}
