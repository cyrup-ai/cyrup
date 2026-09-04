//! EXT-003 (host half) — `project_trust` must stop at the FIRST extension that decides.
//!
//! Pi's `emitProjectTrustEvent` (`coding-agent/src/core/extensions/runner.ts:203-232`) loops the
//! subscribed extensions in load order and `return`s the moment one answers anything other than
//! `"undecided"`. cyrup routed the event through `dispatch_collect_handled`, which deliberately
//! does NOT short-circuit (it is the `resources_discover` aggregator): every subscriber's handler
//! ran, and a later extension's verdict was computed — with whatever side effects that handler has
//! — only to be discarded by `fold_project_trust`.
//!
//! The assertion is about handler INVOCATION, not the returned decision.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    EventKind, ExtError, ExtensionHost, HandledValue, HookOutcome, HostConfig, HostCtx, HostEvent,
    InitApi, NativeExtension,
};
use cyrup_core::{CancelToken, ExtensionId};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A `project_trust` voter with an observable invocation counter.
struct Voter {
    id: &'static str,
    verdict: &'static str,
    calls: AtomicUsize,
}

impl Voter {
    fn new(id: &'static str, verdict: &'static str) -> Arc<Self> {
        Arc::new(Self {
            id,
            verdict,
            calls: AtomicUsize::new(0),
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl NativeExtension for Voter {
    fn id(&self) -> ExtensionId {
        self.id.into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ProjectTrust]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        self.calls.fetch_add(1, Ordering::AcqRel);
        HookOutcome::Handled(HandledValue(json!({ "trusted": self.verdict })))
    }
}

#[tokio::test]
async fn the_chain_stops_at_the_first_extension_that_decides() {
    let host = ExtensionHost::new(HostConfig::default());
    let first = Voter::new("first", "no");
    let second = Voter::new("second", "yes");
    host.load_native(first.clone()).await.expect("load first");
    host.load_native(second.clone()).await.expect("load second");

    let decision = host
        .aggregate_project_trust(&CancelToken::new())
        .await
        .expect("the first extension decided");

    assert!(!decision.trusted, "the FIRST extension's verdict wins");
    assert_eq!(decision.by.to_string(), "first");
    assert_eq!(first.calls(), 1, "the deciding extension ran");
    assert_eq!(
        second.calls(),
        0,
        "an extension after the decider must NOT run at all (Pi runner.ts:203-232 returns \
         immediately) — its handler's side effects would otherwise fire for a discarded verdict"
    );
}

/// An `"undecided"` handler falls through to the next one, and the fall-through handler DOES run.
#[tokio::test]
async fn an_undecided_extension_falls_through_to_the_next() {
    let host = ExtensionHost::new(HostConfig::default());
    let first = Voter::new("first", "undecided");
    let second = Voter::new("second", "yes");
    host.load_native(first.clone()).await.expect("load first");
    host.load_native(second.clone()).await.expect("load second");

    let decision = host
        .aggregate_project_trust(&CancelToken::new())
        .await
        .expect("the second extension decided");
    assert!(decision.trusted);
    assert_eq!(decision.by.to_string(), "second");
    assert_eq!(first.calls(), 1);
    assert_eq!(second.calls(), 1, "the fall-through handler still runs");
}

/// Nobody decides => `None`, and the host falls back to its own saved/default/prompt tiers.
#[tokio::test]
async fn no_decision_when_every_extension_is_undecided() {
    let host = ExtensionHost::new(HostConfig::default());
    let only = Voter::new("only", "undecided");
    host.load_native(only.clone()).await.expect("load");
    assert!(
        host.aggregate_project_trust(&CancelToken::new())
            .await
            .is_none()
    );
    assert_eq!(only.calls(), 1);
}
