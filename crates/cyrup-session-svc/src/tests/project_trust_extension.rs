//! EXT-003 — the `project_trust` event must actually decide the session's trust.
//!
//! Pi resolves project trust through the extensions: `resource-loader.ts:378-399` loads a THROWAWAY
//! pre-trust extension set, awaits `resolveProjectTrust({extensionsResult})`
//! (`main.ts:691-712` → `core/project-trust.ts:46-95`), and slots the extensions' verdict between
//! the `--approve` override and the saved decision. `emitProjectTrustEvent`
//! (`extensions/runner.ts:203-232`) is what actually asks them.
//!
//! cyrup had every piece — `EventKind::ProjectTrust`, the WIT `on-project-trust` export,
//! `ExtensionHost::aggregate_project_trust`, and even `cyrup_config::decide_trust_with_extension`
//! (which slots the verdict at exactly Pi's precedence) — and ZERO production callers for any of
//! them, because trust was frozen in builder step 1 while the `ExtensionHost` was not built until
//! step 4b. These tests assert on the ASSEMBLED SESSION's trust, and on whether the handler ran at
//! all — never on a registration returning `Ok`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::ExtensionId;
use cyrup_ext::{
    EventKind, ExtError, HandledValue, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use serde_json::json;
use tempfile::TempDir;

/// A native built-in that votes on project trust and counts how many times it was asked.
///
/// It also counts `init`s, because a native is a process-lifetime `Arc` (unlike Pi's per-factory-
/// call `Extension`), so the bootstrap pass must not run it through `init` a second time.
struct TrustVoter {
    verdict: &'static str,
    remember: bool,
    asked: Arc<AtomicUsize>,
    inits: Arc<AtomicUsize>,
}

impl TrustVoter {
    fn new(
        verdict: &'static str,
        remember: bool,
    ) -> (Arc<Self>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        let inits = Arc::new(AtomicUsize::new(0));
        let v = Arc::new(TrustVoter {
            verdict,
            remember,
            asked: asked.clone(),
            inits: inits.clone(),
        });
        (v, asked, inits)
    }
}

#[async_trait::async_trait]
impl NativeExtension for TrustVoter {
    fn id(&self) -> ExtensionId {
        "trust-voter".into()
    }
    /// Opting in is what puts this native in the pre-trust bootstrap pass at all; the contract of
    /// the override is "my `init` is idempotent", which this one's is.
    fn decides_project_trust(&self) -> bool {
        true
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        self.inits.fetch_add(1, Ordering::AcqRel);
        api.subscribe(&[EventKind::ProjectTrust]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::ProjectTrust { .. }) {
            return HookOutcome::Noop;
        }
        self.asked.fetch_add(1, Ordering::AcqRel);
        HookOutcome::Handled(HandledValue(
            json!({ "trusted": self.verdict, "remember": self.remember }),
        ))
    }
}

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

/// A cwd that HAS trust-requiring resources — `<cwd>/.cyrup/skills` is one of
/// `has_trust_requiring_resources`'s markers (cyrup-config trust.rs:196-221). Without it Pi's
/// `shouldResolveProjectTrust` guard (main.ts:676-678) correctly declines to ask anyone.
fn fixture_with_trust_requiring_resources() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(cwd.join(".cyrup/skills")).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn fixture_bare() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

async fn build_with(
    fx: &Fixture,
    trust_override: Option<bool>,
    voter: Arc<TrustVoter>,
) -> crate::AgentSession {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.home = fx.agent_dir.clone();
    cfg.trust_override = trust_override;
    SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(voter)
        .build()
        .await
        .unwrap()
}

/// An extension voting `"no"` makes the assembled session UNTRUSTED — the default for this fixture
/// (no saved decision, `defaultProjectTrust` unset) would otherwise have been the decide_trust
/// fallback. The handler must actually have been invoked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_can_deny_project_trust() {
    let fx = fixture_with_trust_requiring_resources();
    let (voter, asked, inits) = TrustVoter::new("no", false);

    let session = build_with(&fx, None, voter).await;

    assert_eq!(
        asked.load(Ordering::Acquire),
        1,
        "the project_trust handler was actually asked"
    );
    // The bootstrap pass and the real load are two DIFFERENT hosts, but the same `Arc`, so `init`
    // runs twice — the hazard `decides_project_trust` exists to make opt-in. Pinned here so the
    // cost is visible to anyone who overrides it.
    assert_eq!(
        inits.load(Ordering::Acquire),
        2,
        "an opted-in native pays a second `init`"
    );
    assert!(
        !session.services().settings.project_trusted(),
        "the extension's `no` verdict decided the assembled session's trust"
    );
}

/// ...and `"yes"` trusts it. Same fixture, opposite verdict: the ONLY difference is the extension's
/// answer, so trust cannot be coming from anywhere else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_can_grant_project_trust() {
    let fx = fixture_with_trust_requiring_resources();
    let (voter, asked, _inits) = TrustVoter::new("yes", false);

    let session = build_with(&fx, None, voter).await;

    assert_eq!(asked.load(Ordering::Acquire), 1);
    assert!(
        session.services().settings.project_trusted(),
        "the extension's `yes` verdict decided the assembled session's trust"
    );
}

/// Pi's `shouldResolveProjectTrust` guard (main.ts:676-678): an explicit `--approve`/`--no-approve`
/// short-circuits before the extensions are consulted at all. Load-bearing — this guard is what
/// keeps the common path from paying for a second extension `init`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_trust_override_never_asks_the_extensions() {
    let fx = fixture_with_trust_requiring_resources();
    let (voter, asked, inits) = TrustVoter::new("no", false);

    let session = build_with(&fx, Some(true), voter).await;

    assert_eq!(
        asked.load(Ordering::Acquire),
        0,
        "an explicit --approve must not consult the extensions"
    );
    assert_eq!(
        inits.load(Ordering::Acquire),
        1,
        "and pays no second `init`"
    );
    assert!(
        session.services().settings.project_trusted(),
        "--approve wins"
    );
}

/// The other half of the guard: a cwd with nothing to gate is trusted without asking anyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_project_with_nothing_to_gate_never_asks_the_extensions() {
    let fx = fixture_bare();
    let (voter, asked, inits) = TrustVoter::new("no", false);

    let session = build_with(&fx, None, voter).await;

    assert_eq!(
        asked.load(Ordering::Acquire),
        0,
        "no trust-requiring resources => no extension pass (Pi main.ts:676-678)"
    );
    assert_eq!(
        inits.load(Ordering::Acquire),
        1,
        "and pays no second `init`"
    );
    assert!(session.services().settings.project_trusted());
}

// =============================================================================================
// The bootstrap pass must not re-`init` a native that did not opt in (EXT-003 blocker).
//
// Pi's second pass calls the extension FACTORY again against a fresh `Extension` + `ExtensionAPI`
// (`loader.ts:148,414-437` — the module cache holds factories, not instances). cyrup has no such
// re-instantiation for a native: a native is a process-lifetime `Arc<dyn NativeExtension>`, so
// running it through the bootstrap pass calls `init` twice ON THE SAME OBJECT. cyrup's own shipped
// natives are not idempotent under that — `cyrup-ext-subagents`' `ChildSafe` arm spawns a detached
// nested-control-inbox poller straight from `init`, and a second poller would race the first over
// the same inbox (each keeps a PRIVATE `seen` set, so both resolve and write back the same
// request). The trigger is the common case: any repo with a `.cyrup/` directory has trust-requiring
// resources, and a subagent child re-execs with no `--approve`.
// =============================================================================================

/// A native that does exactly what the subagents `ChildSafe` arm does: an `init`-time side effect
/// that MUST happen once per process load. It does not override `decides_project_trust`, so it is
/// the default-shaped built-in.
struct NonIdempotentNative {
    inits: Arc<AtomicUsize>,
    /// Stands in for `start_nested_control_inbox_listener`: an unconditional spawn from `init`.
    pollers_started: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for NonIdempotentNative {
    fn id(&self) -> ExtensionId {
        "non-idempotent".into()
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        self.inits.fetch_add(1, Ordering::AcqRel);
        self.pollers_started.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// On a trust-requiring project — the exact case that turns the bootstrap pass on — a native that
/// did not opt in is initialized EXACTLY ONCE. Its `init`-spawned poller is started once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_that_did_not_opt_in_is_initialized_exactly_once() {
    let fx = fixture_with_trust_requiring_resources();
    let inits = Arc::new(AtomicUsize::new(0));
    let pollers = Arc::new(AtomicUsize::new(0));
    let ext = Arc::new(NonIdempotentNative {
        inits: inits.clone(),
        pollers_started: pollers.clone(),
    });

    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.home = fx.agent_dir.clone();
    let _session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(ext)
        .build()
        .await
        .unwrap();

    assert_eq!(
        inits.load(Ordering::Acquire),
        1,
        "the bootstrap pass must not re-`init` a shared native `Arc` — Pi re-invokes a FACTORY, \
         cyrup would be re-initializing the very same object"
    );
    assert_eq!(
        pollers.load(Ordering::Acquire),
        1,
        "exactly one `init`-spawned background poller, not two racing over the same inbox"
    );
}

/// The opt-out must not cost anything else: a non-participating native still loads, and an
/// OPTED-IN one alongside it still decides trust. Proves the filter is per-extension, not a switch
/// that turns the whole pass off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_opted_out_native_does_not_suppress_an_opted_in_voter() {
    let fx = fixture_with_trust_requiring_resources();
    let inits = Arc::new(AtomicUsize::new(0));
    let pollers = Arc::new(AtomicUsize::new(0));
    let bystander = Arc::new(NonIdempotentNative {
        inits: inits.clone(),
        pollers_started: pollers.clone(),
    });
    let (voter, asked, _voter_inits) = TrustVoter::new("no", false);

    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.home = fx.agent_dir.clone();
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(bystander)
        .with_native_extension(voter)
        .build()
        .await
        .unwrap();

    assert_eq!(
        inits.load(Ordering::Acquire),
        1,
        "the bystander was still initialized once"
    );
    assert_eq!(
        asked.load(Ordering::Acquire),
        1,
        "the opted-in voter was still asked"
    );
    assert!(
        !session.services().settings.project_trusted(),
        "and its verdict still decided the session's trust"
    );
}

// ============================================================ SEAM-065: the TIER ORDER ==========

/// SEAM-065 — pi's `resolveProjectTrusted` orders the tiers explicitly
/// (`core/project-trust.ts:46-95` @v0.83.0, identical at v0.84.1):
/// `trustOverride` (`:47`) → no-trust-requiring-resources (`:50`) → **`emitProjectTrustEvent`
/// (`:54-70`)** → the store (`:72-75`) → the default policy (`:77-84`) → `hasUI` (`:86-88`) →
/// `selectProjectTrustOption` → `ctx.ui.select` (`:90-94`). The extension tier returns BEFORE
/// anything else and persists when `result.remember === true`.
///
/// cyrup resolved trust PRE-LAUNCH instead, in `main.rs`'s `resolve_startup_ui`: the human was asked
/// first, the answer became `config.trust_override`, and the builder's
/// `if cfg.trust_override.is_none() && has_resources` guard then skipped
/// `pre_trust_extension_verdict` entirely. So the `on-project-trust` hook was dead on the one path
/// that matters — a policy extension could not stop a user selecting "Trust", could not auto-approve
/// a known-good folder without a prompt, and `remember` never fired.
///
/// This asserts the inversion is gone: with an extension answering, the prompt callback is NEVER
/// invoked. RED before the fix by construction — the callback did not exist, and the prompt ran in
/// `main.rs` ahead of the builder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_verdict_pre_empts_the_prompt_entirely() {
    let fx = fixture_with_trust_requiring_resources();
    let (voter, asked, _inits) = TrustVoter::new("no", false);
    let prompted = Arc::new(AtomicUsize::new(0));

    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.home = fx.agent_dir.clone();
    // Interactive is the ONLY mode whose tiered decision can reach `NeedsPrompt`
    // (`cyrup-config/src/trust.rs`, tier 5), so this is the mode the inversion was live on.
    cfg.app_mode = cyrup_config::AppMode::Interactive;
    let counter = prompted.clone();
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(voter)
        .trust_prompt(Arc::new(move |_options, _saved| {
            counter.fetch_add(1, Ordering::AcqRel);
            Box::pin(async { Some(true) })
        }))
        .build()
        .await
        .unwrap();

    assert_eq!(
        asked.load(Ordering::Acquire),
        1,
        "the extension was consulted"
    );
    assert_eq!(
        prompted.load(Ordering::Acquire),
        0,
        "SEAM-065: an extension verdict returns before `hasUI`/`ui.select` (project-trust.ts:54-70 \
         vs :86-94), so the human must NOT be asked"
    );
    assert!(
        !session.services().settings.project_trusted(),
        "and the extension's `no` is what the session ends up with, not the callback's `yes`"
    );
}

/// The control that keeps the assertion above from being a blanket disarm: with NO extension
/// answering, the tiers fall through to pi's last one and the prompt IS run — and its answer decides
/// (`project-trust.ts:90-94`). Also pins that the callback receives pi's five-row option set,
/// `getProjectTrustOptions(cwd, { includeSessionOnly: true })` (`:32`) — SEAM-064's contract, now
/// owned by the builder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_extension_verdict_the_prompt_runs_and_decides() {
    let fx = fixture_with_trust_requiring_resources();
    let seen_labels: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen_labels.clone();

    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.home = fx.agent_dir.clone();
    cfg.app_mode = cyrup_config::AppMode::Interactive;
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .trust_prompt(Arc::new(move |options, _saved| {
            *sink.lock().unwrap() = options.iter().map(|o| o.label.clone()).collect();
            Box::pin(async { Some(true) })
        }))
        .build()
        .await
        .unwrap();

    assert!(
        session.services().settings.project_trusted(),
        "the prompt's answer decides when no extension answered (project-trust.ts:90-94)"
    );
    let labels = seen_labels.lock().unwrap().clone();
    assert!(
        labels.iter().any(|l| l == "Trust (this session only)")
            && labels
                .iter()
                .any(|l| l == "Do not trust (this session only)"),
        "the pre-launch option set is `includeSessionOnly: true` (project-trust.ts:32) — SEAM-064: \
         {labels:?}"
    );
}

/// A host with no terminal wires no callback, which is pi's `if (!hasUI) return false;`
/// (`project-trust.ts:86-88`) — proceed untrusted rather than hang or guess.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_prompt_callback_is_pis_no_ui_branch() {
    let fx = fixture_with_trust_requiring_resources();
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.home = fx.agent_dir.clone();
    cfg.app_mode = cyrup_config::AppMode::Interactive;
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap();
    assert!(!session.services().settings.project_trusted());
}
