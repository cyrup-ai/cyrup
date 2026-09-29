//! EXT-064 — the PUSH half of pi's `ReadonlyFooterDataProvider`: `onBranchChange`.
//!
//! **Upstream.** `ReadonlyFooterDataProvider` is `Pick<FooterDataProvider, "getGitBranch" |
//! "getExtensionStatuses" | "getAvailableProviderCount" | "onBranchChange">`
//! (`core/footer-data-provider.ts:385-388` @v0.87.1) and is the THIRD argument pi hands the
//! `setFooter` factory (`modes/interactive/interactive-mode.ts:2440`). Three of its four members
//! are reads and ride cyrup's `ui.footer-data` import. The fourth is a subscription:
//!
//! ```ts
//! /** Subscribe to git branch changes. Returns unsubscribe function. */
//! onBranchChange(callback: () => void): () => void {
//!     this.branchChangeCallbacks.add(callback);
//!     return () => this.branchChangeCallbacks.delete(callback);
//! }
//! ```
//! (`core/footer-data-provider.ts:139-143`), fired from `notifyBranchChange` (`:197-199`) — which
//! upstream reaches only inside `if (this.cachedBranch !== undefined && this.cachedBranch !==
//! nextBranch)` (`:224-227`), so an unchanged refresh notifies nobody.
//!
//! **Why cyrup needs it at all.** Upstream's custom footer is a COMPONENT the draw path re-renders,
//! so it re-reads `getGitBranch()` every frame and a branch change repaints it with no extension
//! code involved. A closure cannot cross a component boundary, so a cyrup guest hands over rendered
//! TEXT once — and without this push its footer would freeze at whatever branch was current when it
//! called `set-footer`. That is a behavioural difference, which is why it is ported rather than
//! recorded as a `[CYRUP-DELTA]` beside the signature.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    NativeExtension,
};
use cyrup_core::ExtensionId;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// A native that subscribes and records every branch it is told about, in order.
struct BranchWatcher {
    id: ExtensionId,
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait::async_trait]
impl NativeExtension for BranchWatcher {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe_branch_change();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_branch_change(&self, branch: Option<&str>) {
        if let Ok(mut g) = self.seen.lock() {
            g.push(branch.map(str::to_string));
        }
    }
}

/// A native that subscribes and PANICS, to pin cyrup's containment. See
/// [`crate::ExtensionHost::branch_change`] for why that containment is cyrup-original rather than a
/// port: upstream's `notifyBranchChange` is a bare loop with no `try`/`catch`.
struct PanickingWatcher;

#[async_trait::async_trait]
impl NativeExtension for PanickingWatcher {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("panics")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe_branch_change();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_branch_change(&self, _branch: Option<&str>) {
        panic!("a broken footer extension");
    }
}

/// A native that does NOT subscribe. It must never be called — R-08-034, and upstream's own
/// `branchChangeCallbacks` only holds what `onBranchChange` put in it.
struct Unsubscribed {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for Unsubscribed {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("unsubscribed")
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_branch_change(&self, _branch: Option<&str>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// THE proof: a subscriber is notified, with upstream's `getGitBranch()` TRI-STATE carried
/// verbatim — a branch name, the literal `"detached"`, and `None` outside a repo
/// (`core/footer-data-provider.ts:126-132`).
///
/// **RED without the change:** neither `InitApi::subscribe_branch_change`,
/// `NativeExtension::on_branch_change` nor `ExtensionHost::branch_change` existed, so this file
/// does not compile against the pre-change tree — the surface it exercises is the change. The
/// behavioural half of the row (a guest footer that tracks the branch) is what
/// `cyrup-tui`'s `extension_branch_change` proves end to end.
#[tokio::test]
async fn a_subscribed_extension_is_told_the_new_branch_with_the_tri_state_intact() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(BranchWatcher {
        id: ExtensionId::from("watcher"),
        seen: seen.clone(),
    }))
    .await
    .unwrap();

    assert!(
        host.has_branch_change_subscribers(),
        "the cheap gate sees the subscription (upstream's is `branchChangeCallbacks.size`)"
    );

    host.branch_change(Some("main")).await;
    host.branch_change(Some("detached")).await;
    host.branch_change(None).await;

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec![Some("main".to_string()), Some("detached".to_string()), None,],
        "a branch name, the literal `detached` and `null`-outside-a-repo all reach the guest \
         unflattened (`core/footer-data-provider.ts:126-132` @v0.87.1)"
    );
}

/// The gate: no subscription, no call. Upstream's set only holds what `onBranchChange` added.
#[tokio::test]
async fn an_extension_that_did_not_subscribe_is_never_called() {
    let host = ExtensionHost::new(cfg());
    let calls = Arc::new(AtomicUsize::new(0));
    host.load_native(Arc::new(Unsubscribed {
        calls: calls.clone(),
    }))
    .await
    .unwrap();

    assert!(
        !host.has_branch_change_subscribers(),
        "nothing subscribed, so the poll arm's gate stays shut"
    );
    host.branch_change(Some("main")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Containment, in LOAD order: a panicking subscriber is skipped and the ones after it still run.
///
/// CYRUP-ORIGINAL, and asserted as such rather than as a port. Upstream's `notifyBranchChange` is
/// `for (const cb of this.branchChangeCallbacks) cb();` (`core/footer-data-provider.ts:197-199`
/// @v0.87.1) — no `try`/`catch`, so a throwing listener there stops the ones after it. cyrup needs
/// the containment because a subscriber can be a native built-in whose panic would otherwise unwind
/// through the TUI's poll arm. Being MORE forgiving than upstream costs no parity: nothing a guest
/// can observe here is a behaviour pi would not also produce.
#[tokio::test]
async fn a_panicking_subscriber_is_contained_and_the_rest_still_run() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(PanickingWatcher)).await.unwrap();
    host.load_native(Arc::new(BranchWatcher {
        id: ExtensionId::from("after-the-panic"),
        seen: seen.clone(),
    }))
    .await
    .unwrap();

    host.branch_change(Some("feature/x")).await;

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec![Some("feature/x".to_string())],
        "the subscriber loaded AFTER the panicking one still got its notification"
    );
}
