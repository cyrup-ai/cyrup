//! The event consumer — the half that *listens*, so cyrup's claim on the pane survives
//! contradiction and stops when the pane does.
//!
//! The reporter ([`super::reporter`]) is a write-only loop: it says what cyrup is doing and never
//! finds out what happened to the saying. Two things go wrong without a reader, and both are
//! user-visible:
//!
//! 1. **The pane goes away while cyrup is still running.** A pane the human closes, or whose
//!    process exits, is gone: every later `pane.report_agent` answers `pane_not_found`
//!    (`tmp/herdr/src/api/server.rs:1539`) and the release at shutdown is a connect that can never
//!    succeed. With `pane.closed`/`pane.exited` consumed, the bridge simply stops.
//! 2. **Something else moves the pane's status.** cyrup is its own lifecycle authority
//!    ([`super::SOURCE`]), but herdr's own screen detection and any other reporting source also
//!    write to the same pane record. `pane.agent_status_changed` is herdr telling cyrup that the
//!    pane now reads as something other than what cyrup last reported, and the correct answer is
//!    to re-assert — [`super::state::StateModel::resend`] exists for exactly this.
//!
//! # The no-gap bootstrap, and why it is not optional here
//!
//! herdr documents the ordering (`socket-api.mdx:118-130`): subscribe on **another** connection,
//! wait for the acknowledgement, buffer that stream while `session.snapshot` runs, install the
//! snapshot, apply the buffered events in order, keep streaming; re-snapshot after a reconnect.
//! It is forced rather than advisable, because herdr's socket is one request per connection
//! (`handle_connection_with_stop`, `tmp/herdr/src/api/server.rs:156-317`) and a subscribed
//! connection can never carry a second request. A subscription also does not replay:
//! *"Lifecycle subscriptions start when the request is accepted and do not replay events retained
//! before that point"* (`socket-api.mdx:824-826`).
//!
//! **This file does not re-derive that ordering.** [`cyrup_herdr::HerdrClient::bootstrap`]
//! performs it as one call and [`cyrup_herdr::ReconnectingEvents`] re-performs it behind a backoff
//! when herdr restarts, handing back a fresh [`cyrup_herdr::StreamItem::Snapshot`] before the
//! events of the new stream. Writing the sequence again here would be the second copy of it in the
//! workspace, and the wrong one to keep.
//!
//! # Latency floor
//!
//! herdr polls its subscriptions every `CONNECTION_POLL_INTERVAL = 100 ms`
//! (`tmp/herdr/src/api/server.rs:28`, loop at `:762-778`). That is the delivery floor for
//! everything in this file — design for it, do not fight it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_herdr::schema::{
    AgentStatus, Event, EventData, PaneAgentState, SessionSnapshot, Subscription,
    SubscriptionEventData,
};
use cyrup_herdr::{HerdrClient, ReconnectingEvents, StreamItem};

use super::reporter::Reporter;
use super::state::StateModel;

/// Everything the consumer task owns.
///
/// The pieces are passed individually rather than as a handle to the bridge so that this module
/// has no edge back to [`super::runtime`] — the task outlives nothing it can observe, and a
/// reference cycle through an `Arc` would keep an ended session's bridge alive.
pub struct ConsumerDeps {
    /// The socket, for the two connections `bootstrap` opens.
    pub client: HerdrClient,
    /// This process's pane, from [`cyrup_herdr::HerdrPane::pane_id`].
    pub pane_id: String,
    /// Shared with the edge producers and the reporter; read here, and asked to `resend`.
    pub model: Arc<Mutex<StateModel>>,
    /// Where a re-assertion goes.
    pub reporter: Arc<Reporter>,
    /// Latched when herdr says this pane is gone. [`super::runtime::HerdrBridge`] reads it and
    /// stops producing.
    pub pane_gone: Arc<AtomicBool>,
}

/// The subscriptions cyrup opens, and nothing more.
///
/// A subscription whose `pane_id` does not resolve fails the **whole** subscribe request before
/// the acknowledgement (`tmp/herdr/src/api/server.rs:725-747`; the test at `:1526-1540` asserts
/// `pane_not_found`), so asking for a pane cyrup does not own would take the lifecycle
/// subscriptions down with it.
///
/// `agent_status: None` is deliberate: narrowing to one status reports only transitions **into**
/// it, and a contradiction can be a transition into any of the five.
#[must_use]
pub fn subscriptions(pane_id: &str) -> Vec<Subscription> {
    vec![
        Subscription::pane_agent_status_changed(pane_id, None),
        Subscription::PaneClosed {},
        Subscription::PaneExited {},
    ]
}

/// Run the consumer until the stream ends for good, or the pane does.
///
/// Never returns an error: a herdr that is not there is §12's "in a pane, herdr's server gone"
/// row — log at `debug`, and leave the reporter's own best-effort retries to carry the feature.
pub async fn consume(deps: ConsumerDeps) {
    let (snapshot, mut events) = match ReconnectingEvents::connect(
        deps.client.clone(),
        subscriptions(&deps.pane_id),
    )
    .await
    {
        Ok(pair) => pair,
        Err(err) => {
            tracing::debug!(%err, "herdr: no event stream; the reporter still reports");
            return;
        }
    };
    if apply_snapshot(&deps, &snapshot) == Flow::Stop {
        return;
    }

    while let Some(item) = events.next().await {
        match item {
            // A re-bootstrap after herdr restarted. The whole cache is replaced, which for this
            // consumer means: check the pane still exists, and re-assert cyrup's state — a
            // restarted herdr has never heard of it.
            Ok(StreamItem::Snapshot(snapshot)) => {
                if apply_snapshot(&deps, &snapshot) == Flow::Stop {
                    return;
                }
            }
            Ok(StreamItem::Event(event)) => {
                if apply_event(&deps, &event) == Flow::Stop {
                    return;
                }
            }
            Err(err) => {
                tracing::debug!(%err, "herdr: the event stream is over");
                return;
            }
        }
    }
}

/// Whether the consumer keeps running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    /// Keep reading.
    Continue,
    /// The pane is gone; there is nothing left to observe.
    Stop,
}

/// Install a snapshot: if cyrup's pane is not in it the pane is gone, otherwise re-assert against
/// what herdr currently believes.
fn apply_snapshot(deps: &ConsumerDeps, snapshot: &SessionSnapshot) -> Flow {
    let Some(pane) = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == deps.pane_id)
    else {
        tracing::debug!(
            pane_id = %deps.pane_id,
            "herdr: this pane is not in the session snapshot; the bridge stops"
        );
        mark_pane_gone(deps);
        return Flow::Stop;
    };
    reassert_if_contradicted(deps, &pane.agent_status);
    Flow::Continue
}

/// Route one pushed event.
fn apply_event(deps: &ConsumerDeps, event: &Event) -> Flow {
    match event {
        // The dotted, parameterised envelope — the one `Subscription::PaneAgentStatusChanged`
        // asked for. herdr answers this subscription in its OWN envelope rather than the
        // lifecycle one (`tmp/herdr/src/api/schema/events.rs:377-381`).
        Event::Subscription(envelope) => {
            if let SubscriptionEventData::PaneAgentStatusChanged(changed) = &envelope.data
                && changed.pane_id == deps.pane_id
            {
                reassert_if_contradicted(deps, &changed.agent_status);
            }
            Flow::Continue
        }
        Event::Lifecycle(envelope) => match &envelope.data {
            EventData::PaneClosed { pane_id, .. } | EventData::PaneExited { pane_id, .. }
                if *pane_id == deps.pane_id =>
            {
                tracing::debug!(%pane_id, "herdr: this pane is gone; the bridge stops reporting");
                mark_pane_gone(deps);
                Flow::Stop
            }
            _ => Flow::Continue,
        },
        // A 27th event kind from a newer herdr. Named, not decoded, and never fatal.
        Event::Unrecognised(_) => {
            tracing::trace!(event = %event.name(), "herdr: unrecognised event");
            Flow::Continue
        }
    }
}

/// Latch the pane-gone flag and stop the reporter from speaking to a pane that no longer exists.
///
/// The reporter is *released* rather than merely silenced, because the release is what drops
/// cyrup's authority record. herdr has already discarded the pane, so the call itself will fail —
/// [`Reporter::release`] swallows that, and what matters is that the latch closes both lanes so
/// nothing keeps retrying into a dead pane for the rest of the session.
fn mark_pane_gone(deps: &ConsumerDeps) {
    deps.pane_gone.store(true, Ordering::Release);
    let reporter = Arc::clone(&deps.reporter);
    tokio::spawn(async move { reporter.release().await });
}

/// Re-assert cyrup's own state when herdr's derived status disagrees with it.
///
/// This is the authority half of [`super::SOURCE`]'s doc, made live. It cannot loop: cyrup's own
/// report produces an echo whose status *agrees*, and agreement does nothing.
fn reassert_if_contradicted(deps: &ConsumerDeps, observed: &AgentStatus) {
    let resend = {
        let mut model = deps
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match model.last_reported() {
            // Nothing reported yet — there is nothing to contradict.
            None => None,
            Some(report) if agrees(report.state, observed) => None,
            Some(_) => model.resend(),
        }
    };
    if let Some(report) = resend {
        tracing::debug!(
            ?observed,
            state = ?report.state,
            "herdr: the pane contradicts cyrup's last report; re-asserting"
        );
        deps.reporter.report(report);
    }
}

/// Whether herdr's five-value derived status is consistent with cyrup's four-value reported one.
///
/// `done` is the asymmetry and the reason this is a function rather than an `==`. herdr derives it
/// from *idle and not yet seen* — `pane_agent_status(Idle, seen: false) => Done`
/// (`tmp/herdr/src/app/api_helpers.rs:96-107`), with `seen` flipping when the human looks at the
/// pane's tab (`mark_active_tab_seen`, `tmp/herdr/src/app/actions.rs:522-537`). So a pane reading
/// `done` while cyrup reported `idle` is herdr working exactly as designed, not a contradiction,
/// and treating it as one would make cyrup re-report `idle` every time the human looked away.
#[must_use]
fn agrees(reported: PaneAgentState, observed: &AgentStatus) -> bool {
    match reported {
        PaneAgentState::Idle => matches!(observed, AgentStatus::Idle | AgentStatus::Done),
        PaneAgentState::Working => matches!(observed, AgentStatus::Working),
        PaneAgentState::Blocked => matches!(observed, AgentStatus::Blocked),
        PaneAgentState::Unknown => matches!(observed, AgentStatus::Unknown),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// The three subscriptions, and no fourth. Every extra one is a chance for the whole subscribe
    /// request to fail before the acknowledgement (`tmp/herdr/src/api/server.rs:725-747`).
    #[test]
    fn the_subscription_set_is_the_three_this_bridge_can_act_on() {
        let subs = subscriptions("w1:p1");
        assert_eq!(subs.len(), 3);
        let wire = serde_json::to_value(&subs).unwrap();
        assert_eq!(wire[0]["type"], "pane.agent_status_changed");
        assert_eq!(wire[0]["pane_id"], "w1:p1");
        assert!(
            wire[0].get("agent_status").is_none(),
            "narrowing to one status reports only transitions INTO it"
        );
        assert_eq!(wire[1]["type"], "pane.closed");
        assert_eq!(wire[2]["type"], "pane.exited");
    }

    /// `done` is herdr's own derivation of `idle` + unseen
    /// (`tmp/herdr/src/app/api_helpers.rs:96-107`), so it must NOT read as a contradiction — or
    /// cyrup would re-report `idle` every time the human looked away from the tab.
    #[test]
    fn done_agrees_with_a_reported_idle() {
        assert!(agrees(PaneAgentState::Idle, &AgentStatus::Done));
        assert!(agrees(PaneAgentState::Idle, &AgentStatus::Idle));
        assert!(!agrees(PaneAgentState::Idle, &AgentStatus::Working));
        assert!(!agrees(PaneAgentState::Idle, &AgentStatus::Blocked));
    }

    /// The other three reportable states map one-to-one, `blocked` above all: a pane that reads
    /// anything but `blocked` while cyrup is waiting on the human is the exact failure this whole
    /// feature exists to prevent.
    #[test]
    fn the_other_three_states_agree_only_with_themselves() {
        assert!(agrees(PaneAgentState::Working, &AgentStatus::Working));
        assert!(!agrees(PaneAgentState::Working, &AgentStatus::Idle));
        assert!(!agrees(PaneAgentState::Working, &AgentStatus::Done));
        assert!(agrees(PaneAgentState::Blocked, &AgentStatus::Blocked));
        assert!(!agrees(PaneAgentState::Blocked, &AgentStatus::Working));
        assert!(!agrees(PaneAgentState::Blocked, &AgentStatus::Idle));
        assert!(agrees(PaneAgentState::Unknown, &AgentStatus::Unknown));
        assert!(!agrees(PaneAgentState::Unknown, &AgentStatus::Idle));
    }
}
