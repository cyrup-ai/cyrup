//! still_open[39] — `AgentSessionRuntime::set_before_session_invalidate`, the lifecycle point
//! between `session_shutdown` and session invalidation.
//!
//! Pi ground truth (v0.83.0, `packages/coding-agent/src/core/agent-session-runtime.ts`):
//!
//! ```text
//! :76        private beforeSessionInvalidate?: () => void;
//! :129-131   setBeforeSessionInvalidate(beforeSessionInvalidate?: () => void): void { … }
//! :167-177   private async teardownCurrent(reason, targetSessionFile) {
//!                await this.session.abort();
//!                await emitSessionShutdownEvent(this.session.extensionRunner, {…});
//!                this.beforeSessionInvalidate?.();
//!                this.session.dispose();
//!            }
//! :398-404   async dispose() { await emitSessionShutdownEvent(…, {reason:"quit"});
//!                             this.beforeSessionInvalidate?.(); this.session.dispose(); }
//! ```
//!
//! and pi's own ordering test, `packages/coding-agent/test/agent-session-runtime-events.test.ts`
//! :183-206, asserts the phase array `["session_shutdown", "beforeSessionInvalidate",
//! "rebindSession"]` — i.e. the hook runs strictly after every `session_shutdown` handler has
//! finished and strictly before the outgoing session is invalidated (the pi test proves the latter
//! by reading `oldSession.extensionRunner.createContext()` inside the hook, then asserting the same
//! read THROWS "This extension ctx is stale after session replacement or reload…" afterwards).
//! Pi's sole production consumer is `modes/interactive/interactive-mode.ts:452`,
//! `this.runtimeHost.setBeforeSessionInvalidate(() => { this.resetExtensionUI(); });`.
//!
//! Pre-fix cyrup had no such point at all: `runtime.rs`'s replacement tail was
//! `current.dispose(reason).await; current.notify_replaced(new_gen).await;` with nothing between,
//! and `grep -rn 'before_session_invalidate|beforeSessionInvalidate' crates/ --include=*.rs`
//! returned zero hits. The only hook `install_inner` carried was `before_start` — the mirror-image
//! position, AFTER the new session is installed.
//!
//! The generation watch is NOT a substitute, which is why these tests read it inside the hook: it
//! is an after-the-fact notification (cyrup's stand-in for pi's `setRebindSession`, and what
//! `cyrup-tui`'s `rebind_session` is driven by), so a watcher only wakes once the outgoing session
//! is already gone.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_core::ExtensionId;
use cyrup_ext::{EventKind, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use crate::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use tempfile::TempDir;

// ------------------------------------------------------------------------------- harness ----

type Phases = Arc<Mutex<Vec<String>>>;

/// Records `session_start`/`session_shutdown` in arrival order — the exact surface pi's extensions
/// observe, and the two events the hook must be sandwiched between.
#[derive(Clone)]
struct PhaseRecorder {
    phases: Phases,
}

#[async_trait::async_trait]
impl NativeExtension for PhaseRecorder {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("phase-recorder")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionStart, EventKind::SessionShutdown]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let label = match ev {
            HostEvent::SessionStart { reason, .. } => Some(format!("session_start:{reason}")),
            HostEvent::SessionShutdown { reason, .. } => Some(format!("session_shutdown:{reason}")),
            _ => None,
        };
        if let Some(label) = label
            && let Ok(mut g) = self.phases.lock()
        {
            g.push(label);
        }
        HookOutcome::Noop
    }
}

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.persist = false;
    cfg
}

/// Build a runtime whose only extension is a [`PhaseRecorder`], and hand back the shared log.
async fn runtime_with_recorder(fx: &Fixture) -> (Arc<AgentSessionRuntime>, Phases) {
    let phases: Phases = Arc::new(Mutex::new(Vec::new()));
    let rec = PhaseRecorder { phases: phases.clone() };
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(
        SessionFactory::new(provider, base_config(fx)).with_native_extension(Arc::new(rec)),
    );
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();
    // Drop the initial `session_start{startup}` so each test reads only its own teardown window —
    // pi's test does the same with `events.length = 0`.
    phases.lock().unwrap().clear();
    (runtime, phases)
}

fn log(phases: &Phases) -> Vec<String> {
    phases.lock().unwrap().clone()
}

// ------------------------------------------------------------------------- the headline ----

/// THE proof, and the direct analog of pi's `["session_shutdown", "beforeSessionInvalidate",
/// "rebindSession"]` (test/agent-session-runtime-events.test.ts:201).
///
/// Two independent facts are asserted about the hook's position, because either one alone is
/// satisfiable by a wrong placement:
///
/// 1. **After every `session_shutdown` handler.** The extension log must read
///    `session_shutdown:new` BEFORE `before_session_invalidate`. A hook fired before the notify
///    (or racing it) would fail here.
/// 2. **Before the outgoing session is invalidated.** The hook reads the runtime's generation
///    watch synchronously — `*rx.borrow()`, no await, because the hook is `Fn()` exactly so it
///    cannot yield — and must still see the OLD generation. A hook fired from the install tail
///    (pi's `beforeSessionStart` position, which cyrup already had) would observe the bumped
///    generation and fail here.
#[tokio::test]
async fn hook_runs_after_session_shutdown_and_before_the_session_is_invalidated() {
    let fx = fixture();
    let (runtime, phases) = runtime_with_recorder(&fx).await;

    assert_eq!(runtime.generation().await, 0, "precondition: the initial session is generation 0");

    let gen_at_hook: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let p = phases.clone();
        let g = gen_at_hook.clone();
        let rx = runtime.watch_generation();
        runtime
            .set_before_session_invalidate(Some(Arc::new(move || {
                p.lock().unwrap().push("before_session_invalidate".to_string());
                g.lock().unwrap().push(*rx.borrow());
            })))
            .await;
    }

    runtime.new_session().await.unwrap();

    assert_eq!(
        log(&phases),
        vec![
            "session_shutdown:new".to_string(),
            "before_session_invalidate".to_string(),
            "session_start:new".to_string(),
        ],
        "the host hook must run after the outgoing session's session_shutdown handlers finish and \
         before the replacement announces itself (Pi agent-session-runtime.ts:171-177; pi's own \
         phase assertion is test/agent-session-runtime-events.test.ts:201)"
    );
    assert_eq!(
        *gen_at_hook.lock().unwrap(),
        vec![0_u64],
        "the hook must observe the OLD generation — it runs while the outgoing session is still \
         installed and live, which is the whole point of the callback being synchronous \
         (Pi agent-session-runtime.ts:122-127)"
    );
    assert_eq!(runtime.generation().await, 1, "the replacement did install after the hook ran");
}

/// MIRROR — stays GREEN with or without the fix.
///
/// The same replacement with NO hook registered still orders `session_shutdown` before the
/// replacement's `session_start`. This is the pre-existing behavior the headline test builds on, so
/// it pins down that the headline's failure (when the fire site is removed) comes from the missing
/// `before_session_invalidate` entry alone and not from a broken teardown ordering.
#[tokio::test]
async fn mirror_teardown_ordering_without_a_hook_is_unchanged() {
    let fx = fixture();
    let (runtime, phases) = runtime_with_recorder(&fx).await;

    runtime.new_session().await.unwrap();

    assert_eq!(
        log(&phases),
        vec!["session_shutdown:new".to_string(), "session_start:new".to_string()],
        "with no hook registered a replacement is still shutdown-then-start"
    );
}

// ------------------------------------------------------------------------ every fire site ----

/// The hook fires on `reload` and on the runtime's own `dispose`.
///
/// These are the two call sites of `AgentSession::dispose_with` — `install_inner` (which every
/// replacement funnels through: `new`/`resume`/`fork`/`import` all reach it via `install`, and
/// `reload` calls it directly) and `AgentSessionRuntime::dispose`. Pi fires the hook from the
/// matching pair, `teardownCurrent` (:176) and `dispose` (:403).
///
/// `reload` is the one path with no pi counterpart in `teardownCurrent`, because pi's `reload` is
/// session-tier and keeps the same `AgentSession` object (agent-session.ts). Its interactive host
/// therefore calls `this.resetExtensionUI()` by hand at the top of `handleReloadCommand`
/// (interactive-mode.ts:5340). cyrup's `reload` REPLACES the session object, so firing the hook
/// there reproduces pi's net behavior from the single registration.
#[tokio::test]
async fn hook_fires_on_reload_and_on_runtime_dispose() {
    let fx = fixture();
    let (runtime, phases) = runtime_with_recorder(&fx).await;

    let fired = Arc::new(AtomicUsize::new(0));
    {
        let f = fired.clone();
        let p = phases.clone();
        runtime
            .set_before_session_invalidate(Some(Arc::new(move || {
                f.fetch_add(1, Ordering::SeqCst);
                p.lock().unwrap().push("before_session_invalidate".to_string());
            })))
            .await;
    }

    runtime.reload(None).await.unwrap();
    assert_eq!(fired.load(Ordering::SeqCst), 1, "reload replaces the session object → hook fires");
    assert_eq!(
        log(&phases),
        vec![
            "session_shutdown:reload".to_string(),
            "before_session_invalidate".to_string(),
            "session_start:reload".to_string(),
        ],
        "reload keeps the same sandwich as a replacement"
    );

    phases.lock().unwrap().clear();
    runtime.dispose().await;
    assert_eq!(
        fired.load(Ordering::SeqCst),
        2,
        "runtime.dispose() repeats teardownCurrent's tail (Pi agent-session-runtime.ts:398-404)"
    );
    assert_eq!(
        log(&phases),
        vec!["session_shutdown:quit".to_string(), "before_session_invalidate".to_string()],
        "on quit the hook still lands after session_shutdown, with no replacement to announce"
    );
}

// ---------------------------------------------------------------------- set / clear / swap ----

/// The setter is last-writer-wins and clearable with `None` — pi's
/// `setBeforeSessionInvalidate(undefined)` (test/agent-session-runtime-events.test.ts:205).
#[tokio::test]
async fn hook_is_clearable_and_replaceable() {
    let fx = fixture();
    let (runtime, _phases) = runtime_with_recorder(&fx).await;

    let first = Arc::new(AtomicUsize::new(0));
    let second = Arc::new(AtomicUsize::new(0));

    {
        let f = first.clone();
        runtime
            .set_before_session_invalidate(Some(Arc::new(move || {
                f.fetch_add(1, Ordering::SeqCst);
            })))
            .await;
    }
    runtime.new_session().await.unwrap();
    assert_eq!(first.load(Ordering::SeqCst), 1, "the registered hook fired");

    // Cleared: the next teardown must not call it.
    runtime.set_before_session_invalidate(None).await;
    runtime.new_session().await.unwrap();
    assert_eq!(first.load(Ordering::SeqCst), 1, "a cleared hook is not called again");

    // Replaced: only the new one fires.
    {
        let s = second.clone();
        runtime
            .set_before_session_invalidate(Some(Arc::new(move || {
                s.fetch_add(1, Ordering::SeqCst);
            })))
            .await;
    }
    runtime.new_session().await.unwrap();
    assert_eq!(second.load(Ordering::SeqCst), 1, "the replacement hook fired");
    assert_eq!(first.load(Ordering::SeqCst), 1, "the replaced hook stayed replaced");
}

/// A hook that re-enters `set_before_session_invalidate` must not deadlock against the lock its own
/// invocation was read from — the reason the fire path clones the `Arc` out of the read guard
/// instead of calling under it. A host that tears its own UI down and then unregisters (or swaps in
/// a different teardown for the next generation) does exactly this.
#[tokio::test]
async fn hook_may_reenter_the_setter_without_deadlocking() {
    let fx = fixture();
    let (runtime, _phases) = runtime_with_recorder(&fx).await;

    let fired = Arc::new(AtomicUsize::new(0));
    {
        let f = fired.clone();
        let weak = Arc::downgrade(&runtime);
        runtime
            .set_before_session_invalidate(Some(Arc::new(move || {
                f.fetch_add(1, Ordering::SeqCst);
                // Re-enter from inside the hook. The hook is sync, so hop onto the runtime's
                // current handle rather than awaiting here.
                if let Some(rt) = weak.upgrade() {
                    let handle = tokio::runtime::Handle::current();
                    handle.spawn(async move {
                        rt.set_before_session_invalidate(None).await;
                    });
                }
            })))
            .await;
    }

    tokio::time::timeout(std::time::Duration::from_secs(30), runtime.new_session())
        .await
        .expect("teardown must not deadlock on the hook's lock")
        .unwrap();
    assert_eq!(fired.load(Ordering::SeqCst), 1);
}
