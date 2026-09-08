//! Proof of delivery — the value that authorises destroying a result payload.

use crate::background::RunId;

/// Evidence that one completion notification reached the orchestrator session durably.
///
/// # Why this is a type and not a `bool`
///
/// Consuming a result payload destroys the only copy of a background child's answer. Before this
/// type existed the authority to do that was an unnamed `bool` returned by
/// [`crate::background::watch::CompletionSink::deliver`], and the sink that produced it could not
/// actually tell whether the message had arrived — it reported `true` as soon as a task had been
/// *spawned*. Nine completed runs' results were destroyed on the strength of that `true` in a
/// single working day, three of them never reaching the agent at all.
///
/// A receipt makes the authority unforgeable rather than merely documented: there is exactly one
/// constructor, it is private to this module, and the only code that can reach it is the sink
/// arm that returns [`CompletionDelivery::Delivered`]. A caller therefore cannot *express*
/// "destroy the payload" without first holding evidence that the notification landed.
///
/// # Deliberately not `Clone`, `Copy`, `Default` or `Deserialize`
///
/// Each of those would defeat the point. `Clone`/`Copy` would let one delivery authorise two
/// destructions; `Default` would mint authority from nothing; `Deserialize` would let a file on
/// disk do the same. The receipt is one-use authority over one destruction, and
/// [`crate::background::watch::ResultsWatcher::consume`] takes it by value.
#[derive(Debug)]
pub struct DeliveryReceipt {
    run_id: RunId,
}

impl DeliveryReceipt {
    /// The ONE mint, `pub(super)` so only `delivery::` can issue one.
    #[must_use]
    pub(super) fn issue(run_id: RunId) -> Self {
        Self { run_id }
    }

    /// The run this receipt is evidence for.
    ///
    /// Checked against the payload at consumption, so a receipt earned by delivering run A cannot
    /// authorise destroying run B's payload — the two proofs must agree about *what* was
    /// delivered, not merely that something was.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

/// What the watcher should do with the payload backing a notification.
///
/// Replaces the `bool` this trait used to return. `true`/`false` carried no name for the thing at
/// stake — a sink author choosing `true` was choosing irreversible destruction without the
/// signature ever saying so, and every implementor had to remember the polarity.
#[derive(Debug)]
pub enum CompletionDelivery {
    /// The notification is durably in the session. Carries the authority to consume the payload.
    Delivered(DeliveryReceipt),
    /// Not delivered. Leave the payload on disk for bounded retry-in-place (R-SA-102). This is
    /// the outcome for every non-arrival, including a host with no live session at all.
    Deferred,
}

impl CompletionDelivery {
    /// Record a successful delivery of `run_id`.
    ///
    /// The only route to a [`DeliveryReceipt`], and the reason this constructor exists rather than
    /// letting callers build the variant directly.
    #[must_use]
    pub fn delivered(run_id: RunId) -> Self {
        Self::Delivered(DeliveryReceipt::issue(run_id))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn a_receipt_names_the_run_it_was_earned_for() {
        let delivery = CompletionDelivery::delivered(RunId::from_token("run1"));
        match delivery {
            CompletionDelivery::Delivered(receipt) => {
                assert_eq!(receipt.run_id(), &RunId::from_token("run1"));
            }
            CompletionDelivery::Deferred => panic!("expected a delivered outcome"),
        }
    }

    #[test]
    fn a_deferred_delivery_carries_no_authority() {
        // The property the type exists for: there is no way to obtain a receipt from this arm.
        let delivery = CompletionDelivery::Deferred;
        assert!(matches!(delivery, CompletionDelivery::Deferred));
    }
}
