//! `/subagents-detach` — hand a live foreground single run to the background without killing its
//! child (pi `slash-commands.ts:978-1005`).
//!
//! **Signature frozen by the orchestrator; the body is this batch's work.** Every sentence this
//! handler renders, the reason vocabulary, and the accept-or-refuse handshake it drives all live
//! in [`crate::extension::executor::detach`] — this function resolves a target, calls
//! [`crate::extension::executor::detach::ForegroundDetachHandle::request`], and maps the result
//! onto those constants. It invents no strings of its own.

use crate::background::RunMode;
use crate::error::SubagentError;
use crate::extension::executor::detach::{
    DETACH_NONE_LIVE_MESSAGE, DETACH_NOT_SINGLE_MESSAGE, DetachReason, DetachRefusal, DetachTarget,
    detach_ambiguous_message, detach_no_match_message, detach_refused_message,
    detach_success_message,
};
use crate::extension::host::SubagentsExtension;

impl SubagentsExtension {
    /// pi `detachForegroundRun(args, ctx)` (`slash-commands.ts:978`). The keybinding
    /// (`:1007-1012`) calls the same function with an empty `args`.
    ///
    /// # The two severities, and how they map onto this signature
    ///
    /// Upstream notifies at two levels and cyrup's slash surface carries that distinction as
    /// `Ok`/`Err`, exactly as `/subagents-refine` already does:
    ///
    /// | upstream | severity | here |
    /// |---|---|---|
    /// | `selectForegroundDetachControl`'s throw (`:241`, rendered `:982`) | `error` | `Err` |
    /// | *"No active foreground …"* (`:987`) | `info` | `Ok` |
    /// | *"… supports single-subagent runs only."* (`:990`) | `error` | `Err` |
    /// | *"… is not currently detachable."* (`:993`) | `info` | `Ok` |
    /// | the success sentence (`:999`) | `sendSlashText` | `Ok` |
    ///
    /// # Why every refusal is logged with its reason
    ///
    /// [`DetachRefusal`] names which of upstream's four guards fired
    /// (`execution.ts:613`), where upstream collapses all four into a bare `false`. The
    /// model-facing sentence stays upstream's — one sentence, byte-identical — but a diagnostic
    /// that cannot say WHY a detach was refused is the tool-that-lies this project refuses
    /// elsewhere, so the reason goes to the log beside it.
    pub(crate) async fn slash_subagents_detach(&self, args: &str) -> Result<String, SubagentError> {
        // pi `const id = args.trim()` (`:979`). An empty string is upstream's "no id given", which
        // is the branch the keybinding always takes.
        let requested = args.trim();

        // pi `:980-985` — the selector, whose ambiguity throw the caller catches and renders as an
        // `error` notify.
        let run_id = match self.executor.resolve_foreground_detach_target(requested) {
            DetachTarget::Resolved(run_id) => run_id,
            // pi `:987`'s no-id arm.
            DetachTarget::NoneLive => return Ok(DETACH_NONE_LIVE_MESSAGE.to_string()),
            // pi `:987`'s with-id arm. The requested token comes back on the variant rather than
            // being re-read from `args`, so the sentence can only ever quote what the resolver
            // actually failed to match.
            DetachTarget::NoMatch(requested) => return Ok(detach_no_match_message(&requested)),
            // pi `:241`'s throw, `:982`'s catch.
            DetachTarget::Ambiguous(matched) => {
                return Err(SubagentError::Management(detach_ambiguous_message(
                    requested, &matched,
                )));
            }
        };

        let Some((mode, handle)) = self.executor.foreground_detach_control(&run_id) else {
            // The entry settled between the resolve and this read. Upstream cannot observe this
            // window — its selector and its `control.mode`/`control.detach` reads are one
            // synchronous expression on one event loop — but cyrup's map is shared with a driver
            // running on another task, so the window is real and the honest answer is upstream's
            // own `sessionSettled` refusal.
            tracing::debug!(
                run_id = %run_id,
                refusal = DetachRefusal::SessionSettled.as_str(),
                "/subagents-detach: the run settled while it was being resolved"
            );
            return Ok(detach_refused_message(&run_id));
        };

        // pi `:989-992` — an ERROR notify, and the only branch that is one for a run that exists.
        if mode != RunMode::Single {
            return Err(SubagentError::Management(
                DETACH_NOT_SINGLE_MESSAGE.to_string(),
            ));
        }

        let Some(handle) = handle else {
            // pi `control.detach?.()` with `detach === undefined` (`:993`) — the optional-call
            // short-circuits to `undefined`, which is falsy, so upstream renders exactly the
            // refusal sentence below rather than throwing. A `None` handle here is the same fact:
            // this control surface publishes no detach-ready attempt, because nothing ever called
            // upstream's `onDetachReady` (`execution.ts:1372`) for it — which in cyrup means a
            // control registered by a path other than
            // [`SubagentExecutor::register_foreground_controls`](crate::extension::executor::SubagentExecutor).
            //
            // [`DetachRefusal::LifecycleFinished`] is the right one of the four: it is the only
            // guard that does not ASSERT something we have not observed (the run is not known to
            // be already detached, already settled, or already aborted) while still saying the
            // true thing — there is no live attempt coordinator to accept the hand-off.
            tracing::debug!(
                run_id = %run_id,
                refusal = DetachRefusal::LifecycleFinished.as_str(),
                "/subagents-detach: the run publishes no detach-ready attempt"
            );
            return Ok(detach_refused_message(&run_id));
        };

        // pi `if (!control.detach?.())` (`:993`). `await` because cyrup's verdict travels from the
        // driver's task; see [`ForegroundDetachHandle::request`]'s own doc.
        //
        // [`ForegroundDetachHandle::request`]: crate::extension::executor::detach::ForegroundDetachHandle::request
        match handle.request(DetachReason::UserRequest).await {
            // pi `:999` — the success sentence, which names both recovery verbs.
            Ok(()) => Ok(detach_success_message(&run_id)),
            Err(refusal) => {
                tracing::debug!(
                    run_id = %run_id,
                    refusal = refusal.as_str(),
                    "/subagents-detach: the run's driver refused the hand-off"
                );
                Ok(detach_refused_message(&run_id))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::extension::executor::detach::DetachGate;
    use crate::extension::executor::notices::ForegroundControlEntry;
    use crate::registration::SubagentExtensionConfig;

    /// An extension over a fresh executor seeded with the given live foreground controls.
    fn extension_with(controls: Vec<(&str, ForegroundControlEntry)>) -> SubagentsExtension {
        let extension = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            std::path::PathBuf::from("/tmp"),
        );
        for (run_id, entry) in controls {
            extension
                .executor()
                .insert_foreground_control_for_test(run_id, entry);
        }
        extension
    }

    fn single(updated_at: i64, detach: Option<&DetachGate>) -> ForegroundControlEntry {
        ForegroundControlEntry::for_test(
            RunMode::Single,
            updated_at,
            detach.map(DetachGate::handle),
        )
    }

    /// Each of the five sentences, rendered for the outcome that produces it.
    ///
    /// **Gutting mutation this fails on:** collapse any two branches onto one message — render
    /// the not-found sentence for an ambiguous prefix, drop the `mode` check, or answer a refusal
    /// with the success sentence. The bytes change and the matching assert fires.
    #[tokio::test]
    async fn the_handler_renders_each_sentence_for_its_own_outcome() {
        // (1) nothing live, no id — an `info` (pi `:987`).
        let empty = extension_with(vec![]);
        assert_eq!(
            empty.slash_subagents_detach("").await.unwrap(),
            "No active foreground single-subagent run to detach."
        );
        // (2) an id that matches nothing — an `info` (pi `:987`), quoting the TRIMMED token.
        assert_eq!(
            empty.slash_subagents_detach(" zzz ").await.unwrap(),
            "No active foreground run found for 'zzz'."
        );

        // (3) an ambiguous prefix — an ERROR (pi `:241` thrown, `:982` rendered).
        let two = extension_with(vec![
            ("run-alpha", single(10, None)),
            ("run-beta", single(20, None)),
        ]);
        let err = two
            .slash_subagents_detach("run-")
            .await
            .expect_err("an ambiguous prefix is an error notify upstream");
        assert_eq!(
            err.to_string(),
            "Ambiguous foreground run id prefix 'run-' matched: run-alpha, run-beta. Provide a \
             longer id."
        );

        // (4) a non-single run named by id — an ERROR (pi `:990`).
        let chain = extension_with(vec![(
            "run-chain",
            ForegroundControlEntry::for_test(RunMode::Chain, 10, None),
        )]);
        let err = chain
            .slash_subagents_detach("run-chain")
            .await
            .expect_err("a non-single run is an error notify upstream");
        assert_eq!(
            err.to_string(),
            "/subagents-detach currently supports single-subagent runs only."
        );

        // (5) a single run whose driver already closed the gate — an `info` (pi `:993`).
        let gate = DetachGate::new();
        gate.close(DetachRefusal::SessionSettled);
        let settled = extension_with(vec![("run-x", single(10, Some(&gate)))]);
        assert_eq!(
            settled.slash_subagents_detach("run-x").await.unwrap(),
            "Foreground run run-x is not currently detachable."
        );
    }

    /// A control with no detach-ready attempt REFUSES; it does not panic and it does not hang.
    ///
    /// **Gutting mutation this fails on:** `expect`/`unwrap` the `Option<ForegroundDetachHandle>`
    /// — the test panics instead of returning upstream's refusal sentence.
    #[tokio::test]
    async fn a_control_with_no_detach_handle_refuses_rather_than_panicking() {
        let no_handle = extension_with(vec![("run-y", single(10, None))]);
        assert_eq!(
            no_handle.slash_subagents_detach("run-y").await.unwrap(),
            "Foreground run run-y is not currently detachable."
        );
    }

    /// The accepted path renders the success sentence for the RESOLVED id, not for the prefix the
    /// caller typed.
    ///
    /// **Gutting mutation this fails on:** render `requested` instead of the resolved run id —
    /// the sentence then names `run-li` and all three asserts fire.
    #[tokio::test]
    async fn an_accepted_detach_renders_the_success_sentence_for_the_resolved_id() {
        let gate = DetachGate::new();
        let extension = extension_with(vec![("run-live", single(10, Some(&gate)))]);
        // Stand in for the driver: accept as soon as the request lands.
        let driver = tokio::spawn(async move {
            gate.requested().await;
            gate.accept();
        });
        let text = extension.slash_subagents_detach("run-li").await.unwrap();
        driver.await.expect("driver task");

        assert!(
            text.starts_with("Detached foreground run run-live without terminating its child."),
            "got: {text}"
        );
        assert!(
            text.contains("bg_wait({ id: \"run-live\" })"),
            "got: {text}"
        );
        assert!(
            text.contains("subagent({ action: \"status\", id: \"run-live\" })"),
            "got: {text}"
        );
    }
}
