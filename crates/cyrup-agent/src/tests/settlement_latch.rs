//! The run-in-flight latch is ONE fact, not two (func-02 R-02-045..048).
//!
//! `Agent::wait_for_idle`/`Agent::is_running` observe a `watch<bool>`; `Agent::start_run` refuses a
//! second concurrent run with [`AgentError::RunActive`]. Those two must read and write the SAME
//! cell, because every caller in the workspace does `wait_for_idle().await` (or polls `is_running`)
//! and then immediately prompts again — `AgentSession::prompt` on an unbound session propagates the
//! refusal straight out to the caller as `Agent(RunActive)`.
//!
//! Before the fix, `SettlementGuard::drop` published `running_tx.send(false)` and only THEN cleared
//! a separate `active: Arc<Mutex<bool>>`. A caller woken by that very send could reach `start_run`
//! inside the two-statement gap and be refused despite the agent reporting itself idle. The gap is
//! nanoseconds when nothing else is running and milliseconds when the machine is loaded enough to
//! preempt between the two statements — which is exactly why it surfaced as an intermittent
//! `prompt 2: Agent(RunActive)` only inside a full parallel workspace test run.
//!
//! `start_run` now claims the latch with `watch::Sender::send_if_modified` (a compare-and-set under
//! the channel's own write lock) and the guard's `send(false)` is the only release, so an observed
//! `false` and "a new run may start" are the same fact.

use std::sync::Arc;

use crate::{Agent, AgentError, ProviderStreamFn, StreamFn};
use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;

use super::support::model_ref;

/// A faux stream fn that answers every turn identically, so a run costs almost nothing and the
/// settlement edge can be hammered.
fn stream_fn(turns: usize) -> Arc<dyn StreamFn> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(
        (0..turns)
            .map(|_| faux_assistant_message(vec![faux_text("ok")], StopReason::Stop))
            .collect(),
    );
    let provider: Arc<dyn Provider> = faux.clone();
    Arc::new(ProviderStreamFn::new(provider))
}

/// The headline invariant: the instant `is_running()` reads `false`, the very next `prompt` MUST be
/// accepted. A dedicated task pounces on that edge as tightly as the runtime allows, so it lands in
/// the window the old two-flag settlement left open; with a single latch there is no window to land
/// in at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observed_idle_agent_always_accepts_the_next_run() {
    const ROUNDS: usize = 400;

    let agent = Arc::new(Agent::builder(model_ref(), stream_fn(2 * ROUNDS + 8)).build());

    for round in 0..ROUNDS {
        agent.prompt("go").await.unwrap_or_else(|e| {
            panic!("round {round}: the first prompt of a round must be accepted: {e:?}")
        });

        // Pounce on the settlement edge from a task of its own, so the observation and the
        // subsequent claim happen on a DIFFERENT worker than the one running the guard's `drop` —
        // that cross-thread interleaving is the whole failure mode.
        let pouncer = Arc::clone(&agent);
        let result = tokio::spawn(async move {
            loop {
                if !pouncer.is_running() {
                    return pouncer.prompt("go").await.map(|_| ());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the pouncer task itself must not panic");

        match result {
            Ok(()) => {}
            Err(AgentError::RunActive(_)) => panic!(
                "round {round}: `is_running()` reported the agent idle and `prompt` was then \
                 refused with RunActive — the idle observation and the run-start guard are reading \
                 two different cells again"
            ),
            Err(other) => panic!("round {round}: unexpected prompt failure: {other:?}"),
        }

        agent.wait_for_idle().await;
    }
}

/// The complementary half: `wait_for_idle()` resolving is itself a promise that a new run can
/// start. This is the exact sequence `cyrup-session-svc`'s tests (and every one-shot embedder) use
/// — `prompt(); wait_for_idle(); prompt()` — and it must never produce `RunActive`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wait_for_idle_then_prompt_is_never_refused() {
    const ROUNDS: usize = 200;

    let agent = Agent::builder(model_ref(), stream_fn(ROUNDS + 8)).build();

    for round in 0..ROUNDS {
        agent
            .prompt("go")
            .await
            .unwrap_or_else(|e| panic!("round {round}: prompt refused: {e:?}"));
        agent.wait_for_idle().await;
        assert!(
            !agent.is_running(),
            "round {round}: wait_for_idle() resolved while the agent still reports a live run"
        );
    }
}

/// The latch still does its real job: exactly ONE of a burst of concurrent starts wins, and the
/// losers get `RunActive` rather than corrupting the run. The compare-and-set must not have turned
/// the guard into a no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_starts_admit_exactly_one_run() {
    // The first run is held OPEN until every competitor has attempted. Without this gate the test
    // asserts a scheduling accident rather than the latch: `FauxProvider` answers instantly, so
    // under CPU contention task #1 can finish its whole run before task #16 is even scheduled, and
    // the later start is then admitted LEGITIMATELY — sequentially, not concurrently. That is what
    // `accepted: 2` meant when this failed 3 runs in 6 under load while passing 5 in 5 idle.
    //
    // The invariant being tested is "at most one run AT A TIME", and only a run that is still in
    // flight can test it.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let inner = stream_fn(8);
    let gated: Arc<dyn StreamFn> = Arc::new(GatedStreamFn { inner, gate: Arc::clone(&gate) });
    let agent = Arc::new(Agent::builder(model_ref(), gated).build());

    // All 16 start together, and none can settle while the gate is closed.
    let started = Arc::new(tokio::sync::Barrier::new(16));
    // Counts tasks whose `prompt()` has RETURNED. The winner never returns while the gate is shut,
    // so this reaching 15 proves all 16 have called `prompt()` — 15 rejected plus one parked.
    let returned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let a = Arc::clone(&agent);
        let b = Arc::clone(&started);
        let r = Arc::clone(&returned);
        tasks.push(tokio::spawn(async move {
            b.wait().await;
            let outcome = a.prompt("go").await.map(|_| ());
            r.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            outcome
        }));
    }

    // Open the gate ONLY once every competitor has been rejected. Releasing it earlier (as a first
    // attempt at this fix did) reintroduces the exact race: the winner settles, and a later start
    // is admitted legitimately.
    while returned.load(std::sync::atomic::Ordering::SeqCst) < 15 {
        tokio::task::yield_now().await;
    }
    gate.add_permits(1);

    let mut accepted = 0usize;
    for task in tasks {
        match task.await.expect("no task panics") {
            Ok(()) => accepted += 1,
            Err(AgentError::RunActive(_)) => {}
            Err(other) => panic!("unexpected prompt failure: {other:?}"),
        }
    }
    assert_eq!(accepted, 1, "exactly one concurrent start may claim the run latch");

    agent.wait_for_idle().await;
    gate.add_permits(1);
    agent.prompt("go").await.expect("the latch is released once the run settles");
    agent.wait_for_idle().await;
}

/// A [`StreamFn`] whose stream yields nothing until a permit is released, so a run can be held
/// deliberately in flight. `FauxProvider` is otherwise instantaneous, which leaves no window in
/// which concurrent starts are actually concurrent.
struct GatedStreamFn {
    inner: Arc<dyn StreamFn>,
    gate: Arc<tokio::sync::Semaphore>,
}

impl StreamFn for GatedStreamFn {
    fn stream(
        &self,
        model: &cyrup_core::ModelRef,
        ctx: &crate::Context,
        opts: &crate::StreamOptions,
    ) -> cyrup_core::EventStream<cyrup_provider::StreamEvent> {
        let inner = self.inner.stream(model, ctx, opts);
        let gate = Arc::clone(&self.gate);
        Box::pin(
            futures::StreamExt::flatten(futures::stream::once(async move {
                // A permit is never returned to the semaphore: each `add_permits(1)` releases
                // exactly one run.
                if let Ok(p) = gate.acquire().await {
                    p.forget();
                }
                inner
            })),
        )
    }
}
