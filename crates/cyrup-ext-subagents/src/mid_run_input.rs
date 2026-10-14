//! SUBA-192 / SUBA-198 — the ONE admission rule for a message the operator sends while the agent
//! is busy, shared by the two consumers that act on it.
//!
//! The host publishes every submission as [`cyrup_ext::HostEvent::Input`], carrying where it came
//! from (`source`) and, only while a run is active, how it will be queued onto that run
//! (`streaming_behavior`: steer or follow-up; `None` means no queue selector — the agent was idle,
//! or a busy submission named none and is about to be refused with `StreamingNeedsBehavior`). Two
//! parts of this crate must react when the operator types into a BUSY agent:
//!
//! - a blocking `bg_wait` ends with a non-error `user_input` yield, so the turn does not sit out
//!   the window with the message held behind it ([`crate::background::wait::UserInputWaits`]; pi
//!   `runs/background/wait-tool.ts:32-35` @ad11b7ab);
//! - the main watchdog adds the message to its scope record, so work it asked for is not later
//!   flagged as scope drift
//!   ([`crate::watchdog::runtime::MainWatchdogRuntime::handle_user_input`]; pi
//!   `watchdog/runtime.ts:514,516` @ad11b7ab).
//!
//! Upstream spells the rule twice, once inline in each listener (`wait-tool.ts:33`:
//! `event.source === "extension" || !event.streamingBehavior`, and `runtime.ts:514`+`:516`). Here
//! it is written once, in [`admits`], and both consumers call it: two copies are how the two
//! listeners would drift apart.
//!
//! What the rule deliberately does NOT cover: upstream's watchdog also resets an in-flight review
//! on ANY non-extension input, idle or not, when clarification is on (`runtime.ts:515` @ad11b7ab,
//! SUBA-234). That is a WEAKER rule — [`is_operator_input`], not disposed and not extension, with no
//! streaming condition — and it is kept separate rather than folded into [`admits`]: a predicate
//! that admitted idle input would end `bg_wait` on a message that is about to start a new run, and
//! [`admits`] would drop the idle case upstream's own reset test covers
//! (`watchdog-runtime.test.ts:278` @ad11b7ab, `handleUserInput({ text, source: "interactive" })`
//! with no `streamingBehavior`).
//!
//! `[CYRUP-DELTA]` the extension half of the rule is correct but, today, never decides anything in
//! production: the host labels an extension's own `sendUserMessage` as
//! [`cyrup_ext::InputEventSource::Interactive`], not `Extension` (cyrup-session-svc's
//! `InputSource` has no extension variant — `run_user_messages` and the guest
//! `ControlOp::SendUserMessage` arm both pass `InputSource::Sdk`, which the input dispatch maps to
//! `Interactive`). pi passes `source: "extension"` there (`sendUserMessage`,
//! `agent-session.ts:2393` @f1b2e77f5). So a guest extension that calls
//! `sendUserMessage(text, { deliverAs })` while a run is active is admitted here, where pi would
//! skip it. The fix is the host's label — filed as `SEAM-172` against the session service — and
//! this rule is already the right one for when the label lands. The same label decides
//! [`is_operator_input`], so until it lands an extension's mid-review `sendUserMessage` also aborts
//! a clarification-enabled watchdog review that pi would leave running.

use cyrup_ext::{InputEventSource, InputStreamingBehavior};

/// Whether an `input` event is a message the operator sent while the agent was busy — the rule
/// both `bg_wait` and the main watchdog apply before acting on it.
///
/// `true` exactly when the input did NOT come from an extension and carries a
/// `streaming_behavior` (steer or follow-up). Input without a behavior (`None`) is not queued onto
/// the live run: on an idle agent it starts a new run, which the watchdog records at
/// `before_agent_start`; on a busy one it is refused (`SessionServiceError::StreamingNeedsBehavior`)
/// and never reaches the model. Extension input is the extension talking, not the operator.
#[must_use]
pub fn admits(
    source: InputEventSource,
    streaming_behavior: Option<InputStreamingBehavior>,
) -> bool {
    source != InputEventSource::Extension && streaming_behavior.is_some()
}

/// SUBA-234 — whether an `input` event is the operator talking at all, busy or idle: the rule the
/// main watchdog's clarification reset applies (`runtime.ts:514-515` @ad11b7ab: `if (this.disposed
/// || event.source === "extension") return;` then reset when a review is in flight).
///
/// Deliberately weaker than [`admits`]: there is no `streaming_behavior` condition, because the
/// reset is about a REVIEW being in flight, not about a message joining the live run. An operator
/// who types while a boundary review of the finished run is still running (the agent idle) is
/// answering, or moving on from, exactly the work that review is judging; upstream aborts it, and
/// so does this. [`admits`] would answer `false` for that message.
#[must_use]
pub fn is_operator_input(source: InputEventSource) -> bool {
    source != InputEventSource::Extension
}

#[cfg(test)]
mod tests {
    use super::{admits, is_operator_input};

    /// SUBA-234 — the weaker rule's truth table: every non-extension source, with ANY streaming
    /// behavior including none, is the operator; and it differs from [`admits`] exactly on idle
    /// operator input.
    #[test]
    fn operator_input_is_every_non_extension_source_idle_included() {
        for source in [
            InputEventSource::Interactive,
            InputEventSource::Rpc,
            InputEventSource::Extension,
        ] {
            assert_eq!(
                is_operator_input(source),
                source != InputEventSource::Extension,
                "{source:?}"
            );
        }
        assert!(is_operator_input(InputEventSource::Interactive));
        assert!(
            !admits(InputEventSource::Interactive, None),
            "the shared mid-run rule drops the idle case the reset needs"
        );
    }
    use cyrup_ext::{InputEventSource, InputStreamingBehavior};

    /// The whole truth table, both upstream listeners' shared condition
    /// (`wait-tool.ts:33`, `runtime.ts:514,516` @ad11b7ab): every non-extension source is admitted
    /// with either streaming behavior, and nothing is admitted while idle or from an extension.
    #[test]
    fn only_non_extension_input_sent_while_the_agent_streams_is_admitted() {
        let sources = [
            InputEventSource::Interactive,
            InputEventSource::Rpc,
            InputEventSource::Extension,
        ];
        let behaviors = [
            None,
            Some(InputStreamingBehavior::Steer),
            Some(InputStreamingBehavior::FollowUp),
        ];
        for source in sources {
            for behavior in behaviors {
                let expected = source != InputEventSource::Extension && behavior.is_some();
                assert_eq!(
                    admits(source, behavior),
                    expected,
                    "source {source:?}, streaming_behavior {behavior:?}"
                );
            }
        }
    }
}
