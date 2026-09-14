//! SCOPE_11 — the executor half of durable wait subscriptions: the three injected seams
//! [`crate::background::wait_subscriptions`] declares but cannot implement, the manager's
//! lifecycle slot, and the fourth completion-observer member that gives the wake a real edge.
//!
//! # Why any of this lives up here
//!
//! `background/` sits BELOW `extension/executor/`. [`ForegroundControlEntry`] and
//! [`ForegroundHistoryRun`] are both `pub(crate)` inside this directory, and
//! [`cyrup_ext::host::HostServices`] is reached through the executor's own late-bound P-1 slot. A
//! `background/wait_subscriptions/` module can name none of them, which is exactly why the manager
//! takes injected dependencies — the same shape [`crate::background::wait::WaitDeps`] already uses
//! for the completion bus, the completion store and the inline-answer ledger.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::background::RunId;
use crate::background::wait_completions::WaitCompletion;
use crate::background::wait_subscriptions::{
    ForegroundChildState, ForegroundSubscriptionProbe, ForegroundTargetChild,
    ForegroundTargetState, SUBSCRIPTION_MESSAGE_CUSTOM_TYPE, SubscriptionNotifier,
    SubscriptionOutcome, SubscriptionSessions, WaitSubscriptionDeps, WaitSubscriptionManager,
    WaitSubscriptionRecord, subscription_message_content, subscription_message_details,
};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::identity::SessionId;

/// The manager's home on the executor — one slot, shared by every reader.
///
/// `Arc<Mutex<Option<…>>>` rather than a bare `Option` because the completion observer registered
/// into the watcher's composite must reach whatever manager is current at OBSERVATION time, not
/// whichever one existed when the watcher was installed. The two are re-created on independent
/// `SessionStart` edges, and a snapshot taken at install time would go stale on the next one.
pub(crate) type WaitSubscriptionSlot = Arc<Mutex<Option<Arc<WaitSubscriptionManager>>>>;

// =================================================================================================
// The three injected seams
// =================================================================================================

/// pi `state.currentSessionId`, read LIVE off the executor's P-1 host-services slot.
///
/// A [`Weak`] so the manager (which the executor owns) cannot keep the executor alive through its
/// own dependencies. A dropped executor reads as "no session", which is the correct degradation:
/// every gate that consults it then refuses rather than settling into a session that is gone.
struct ExecutorSubscriptionSessions {
    executor: Weak<SubagentExecutor>,
}

impl SubscriptionSessions for ExecutorSubscriptionSessions {
    fn current_session_id(&self) -> Option<SessionId> {
        SessionId::parse_opt(
            Weak::upgrade(&self.executor)?
                .current_session_id()
                .as_deref(),
        )
    }
}

/// pi `state.foregroundRuns?.get(record.runId)` (`:212`, `:329`) over cyrup's TWO disjoint maps.
///
/// # The consultation order, and why it is not a preference
///
/// The maps are disjoint BY CONSTRUCTION (`extension/executor/mod.rs:188-192`: a history entry is
/// created at the exact point the live control entry for the same id is removed), so at most one
/// of them can answer and the order is only about which is checked first.
///
/// 1. **`foreground_controls` — the LIVE run.** Its `active_children` carry
///    `current_activity_state`/`current_tool`, which is what makes `:220`'s `contact_supervisor`
///    test real rather than vacuous. A live child has not settled, so it maps to
///    [`ForegroundChildState::Detached`] — upstream's `detached.length > 0` state, the one that
///    keeps the subscription armed.
/// 2. **`foreground_runs` — the SETTLED run**, cyrup's literal `state.foregroundRuns`. Its
///    `ForegroundHistoryChild::status` supplies `detached`/`failed`/`completed`/`stopped` (minted
///    by `foreground_history_child_status`), which is exactly what `:224-227` needs.
///    `activity_state`/`current_tool` are absent from that record, so `:220` simply cannot fire —
///    correct, because a settled run has no tool in flight.
/// 3. **Neither** → `None` → `:215`'s `!run` arm → `"could not be reconciled"`.
struct ExecutorForegroundProbe {
    executor: Weak<SubagentExecutor>,
}

impl ForegroundSubscriptionProbe for ExecutorForegroundProbe {
    fn probe(&self, run_id: &RunId) -> Option<ForegroundTargetState> {
        let executor = Weak::upgrade(&self.executor)?;
        if let Some(live) = executor
            .foreground_controls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run_id.as_str())
        {
            let mut children: Vec<ForegroundTargetChild> = live
                .active_children
                .values()
                .map(|child| ForegroundTargetChild {
                    state: ForegroundChildState::Detached,
                    activity_state: child.current_activity_state,
                    current_tool: child.current_tool.clone(),
                })
                .collect();
            if children.is_empty() {
                // A live run that has not yet registered a per-child entry is STILL in flight, and
                // reporting zero children here would take upstream's `:224` `detached.length === 0`
                // branch and settle it "completed" the instant it was armed. The entry's own
                // `current_activity_state`/`current_tool` are the single-child mirror pi's
                // `syncCurrentChild` maintains (`foreground-control.ts:39-60`), so the synthesized
                // child is the same run described one level up, not an invention.
                children.push(ForegroundTargetChild {
                    state: ForegroundChildState::Detached,
                    activity_state: live.current_activity_state,
                    current_tool: live.current_tool.clone(),
                });
            }
            return Some(ForegroundTargetState {
                session_id: live.session_id.clone(),
                children,
            });
        }
        let settled = executor
            .foreground_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run_id)
            .cloned()?;
        Some(ForegroundTargetState {
            session_id: Some(settled.session_id.clone()),
            children: settled
                .children
                .iter()
                .map(|child| ForegroundTargetChild {
                    // pi compares the raw status string against `"detached"` and `"failed"` and
                    // lumps everything else into its final `else` (`:219`, `:225-226`).
                    state: match child.status.as_str() {
                        "detached" => ForegroundChildState::Detached,
                        "failed" => ForegroundChildState::Failed,
                        _ => ForegroundChildState::Settled,
                    },
                    // Not persisted — the `[CYRUP-DELTA]` in `wait_subscriptions`' module doc.
                    activity_state: None,
                    current_tool: None,
                })
                .collect(),
        })
    }
}

/// pi `pi.sendMessage({ … }, { triggerTurn: true })` (`:189-199`) over the P-1 capability backend.
///
/// Takes the ACKNOWLEDGED seam ([`cyrup_ext::host::HostServices::inject_message_ack`]), not
/// `inject_message`, for the reason `inject_message`'s own doc gives: *"the host took it" is not
/// "the session has it"*. [`crate::background::watch::HostServicesCompletionSink`] already took the
/// same path for the same reason, and here the stakes are identical — the record this wake is
/// announcing is unlinked the moment `true` comes back.
struct HostServicesSubscriptionNotifier {
    executor: Weak<SubagentExecutor>,
}

#[async_trait::async_trait]
impl SubscriptionNotifier for HostServicesSubscriptionNotifier {
    async fn notify(
        &self,
        record: &WaitSubscriptionRecord,
        outcome: SubscriptionOutcome,
        detail: &str,
        completion: Option<&WaitCompletion>,
    ) -> bool {
        let Some(executor) = Weak::upgrade(&self.executor) else {
            return false;
        };
        let Some(services) = executor.host_services() else {
            // No live backend: nothing was delivered, so the record must stay armed.
            return false;
        };
        let content = subscription_message_content(record, outcome, detail);
        let details = subscription_message_details(record, outcome, completion);
        let Ok(ack) = services.inject_message_ack(
            &content,
            Some(SUBSCRIPTION_MESSAGE_CUSTOM_TYPE),
            // pi `display: true` (`:192`) and `{ triggerTurn: true }` (`:199`): a wake that does
            // not re-enter the turn loop is not a wake.
            true,
            Some(&details),
            true,
        ) else {
            return false;
        };
        // A dropped sender (the backend went away mid-flight) is `Err`, which is exactly
        // "not accepted" — the record stays armed and the next reconcile re-delivers.
        matches!(ack.await, Ok(cyrup_ext::host::InjectOutcome::Accepted))
    }
}

// =================================================================================================
// The wake edge
// =================================================================================================

/// The FOURTH member of the watcher's `CompositeCompletionObserver` — pi's fourth listener on
/// `SUBAGENT_ASYNC_COMPLETE_EVENT`, which `background/watch/observer.rs:38-45` has named since
/// SUBA-034 landed.
///
/// Registered LAST, deliberately: member #1 is
/// [`crate::background::wait_completions::WaitCompletionStore`], and a subscription that reconciles
/// here must find the record the store is about to write already present, or its completion read
/// falls through a rung for no reason. pi registers its four listeners on one event with no
/// documented ordering guarantee, so this is an assertion this port makes rather than one it
/// inherits.
pub(crate) struct WaitSubscriptionCompletionObserver {
    slot: WaitSubscriptionSlot,
}

impl WaitSubscriptionCompletionObserver {
    pub(crate) fn new(slot: WaitSubscriptionSlot) -> Self {
        Self { slot }
    }
}

#[async_trait::async_trait]
impl crate::background::watch::CompletionObserver for WaitSubscriptionCompletionObserver {
    async fn observe(
        &self,
        _notification: &crate::background::watch::CompletionNotification,
    ) -> bool {
        // The whole run set is re-reconciled rather than just the completing run: `reconcile` also
        // settles timeouts and sweeps foreign records, and a completion edge is as good a moment
        // as any to do both. It is the same "the event is a WAKE-UP, never the source of truth"
        // contract `CompletionBus` states for itself.
        let manager = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(manager) = manager {
            manager.reconcile().await;
        }
        // An observer's `bool` reports whether the OBSERVATION succeeded, and reconciliation is
        // internally best-effort by contract (every failure settles or is logged), so there is no
        // failure to report. Returning `false` here would retain the cross-session mission
        // observer index for a retry that has nothing to retry.
        true
    }
}

// =================================================================================================
// Lifecycle
// =================================================================================================

impl SubagentExecutor {
    /// A handle on this session's subscription manager, or `None` when none is installed — a
    /// headless runtime, or a process before its first `SessionStart`.
    ///
    /// `None` is what makes `{ nonBlocking: true }` take pi's `!deps.subscribe` refusal
    /// (`subagent-wait.ts:591-593`).
    #[must_use]
    pub fn wait_subscriptions(&self) -> Option<Arc<WaitSubscriptionManager>> {
        self.wait_subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The slot itself, for the completion observer that must resolve the manager LATE. Cloning
    /// shares the one cell, mirroring [`Self::completion_bus`].
    pub(crate) fn wait_subscription_slot(&self) -> WaitSubscriptionSlot {
        Arc::clone(&self.wait_subscriptions)
    }

    /// pi `createWaitSubscriptionManager(pi, state)` + `waitSubscriptionManager.restore()`
    /// (`extension/index.ts:493` and `:971`), collapsed onto cyrup's one `SessionStart` edge.
    ///
    /// Upstream creates the manager once at extension load and restores per session. cyrup builds
    /// it here instead, for two reasons the load-time shape cannot satisfy: the subscriptions
    /// directory is per-`cwd` (see [`crate::background::wait_subscriptions`]'s directory decision)
    /// and `cwd` is only known from a [`cyrup_ext::native::HostCtx`]; and the 1 s reconcile timer
    /// is a `tokio` task, which needs a runtime the executor's constructor does not run in.
    /// Re-entrant across sessions: an existing manager is DISPOSED and replaced, so the old timer
    /// cannot keep reconciling against a directory this session no longer uses.
    pub async fn install_wait_subscriptions(
        self: &Arc<Self>,
        cwd: &std::path::Path,
    ) -> Arc<WaitSubscriptionManager> {
        let roots = self.config_snapshot().await.roots;
        let manager = WaitSubscriptionManager::new(WaitSubscriptionDeps {
            subscriptions_dir: crate::background::wait_subscriptions_dir_in(&roots, cwd),
            async_root: default_async_root_in(&roots, cwd),
            results_dir: default_results_dir_in(&roots, cwd),
            sessions: Arc::new(ExecutorSubscriptionSessions {
                executor: Arc::downgrade(self),
            }),
            notifier: Arc::new(HostServicesSubscriptionNotifier {
                executor: Arc::downgrade(self),
            }),
            foreground: Arc::new(ExecutorForegroundProbe {
                executor: Arc::downgrade(self),
            }),
            // The SAME store the `wait` tool reads through, so a subscription and a blocking wait
            // can never disagree about what a run reported.
            wait_completions: self.wait_completions(),
        });
        if let Some(previous) = self
            .wait_subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(Arc::clone(&manager))
        {
            previous.dispose();
        }
        // pi `:971` — restore is what re-reads this session's records off disk, arms the timer and
        // performs the first reconcile.
        manager.restore().await;
        manager
    }

    /// pi `waitSubscriptionManager.dispose()` (`extension/index.ts:1009`), on `SessionShutdown`.
    ///
    /// Clears the slot as well as disposing, so a shut-down session cannot arm a new subscription
    /// through a manager whose timer is already gone.
    pub fn dispose_wait_subscriptions(&self) {
        if let Some(manager) = self
            .wait_subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            manager.dispose();
        }
    }
}
